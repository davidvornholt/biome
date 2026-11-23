use crate::JsRuleAction;
use crate::services::semantic::Semantic;
use crate::shared::node_process::is_process_module_import;
use biome_analyze::{
    FixKind, Rule, RuleDiagnostic, RuleSource, context::RuleContext, declare_lint_rule,
};
use biome_console::markup;
use biome_js_factory::make;
use biome_js_semantic::Binding;
use biome_js_syntax::AnyJsExpression;
use biome_js_syntax::AnyJsMemberExpression;
use biome_js_syntax::JsDefaultImportSpecifier;
use biome_js_syntax::JsImport;
use biome_js_syntax::JsImportDefaultClause;
use biome_js_syntax::JsLanguage;
use biome_js_syntax::JsModuleItemList;
use biome_js_syntax::JsSyntaxKind;
use biome_js_syntax::JsUnaryExpression;
use biome_js_syntax::JsUnaryOperator;
use biome_js_syntax::T;
use biome_js_syntax::{AnyPossibleGlobalIdentifier, global_identifier};
use biome_rowan::AstNode;
use biome_rowan::BatchMutation;
use biome_rowan::BatchMutationExt;
use biome_rowan::SyntaxElement;
use biome_rowan::SyntaxNode;
use biome_rowan::TriviaPieceKind;
use biome_rule_options::no_process_global::NoProcessGlobalOptions;
use smallvec::SmallVec;
use smallvec::smallvec;

declare_lint_rule! {
    /// Disallow the use of `process` global.
    ///
    /// Node.js and Deno expose `process` global but they are hard to statically analyze by tools,
    /// so code should not assume they are available. Instead, `import process from "node:process"`.
    ///
    ///
    /// ## Examples
    ///
    /// ### Invalid
    ///
    /// ```js,expect_diagnostic
    /// const foo = process.env.FOO;
    /// ```
    ///
    /// ### Valid
    ///
    /// ```js
    /// import process from "node:process";
    ///
    /// const foo = process.env.FOO;
    /// ```
    ///
    /// The rule is not able to detect cases where the global object is aliased:
    ///
    /// ```js
    /// const foo = globalThis;
    /// const bar = foo.process;
    /// ```
    ///
    pub NoProcessGlobal {
        version: "2.0.0",
        name: "noProcessGlobal",
        language: "js",
        sources: &[RuleSource::DenoLint("no-process-global").same()],
        recommended: false,
        fix_kind: FixKind::Safe,
    }
}

impl Rule for NoProcessGlobal {
    type Query = Semantic<AnyPossibleGlobalIdentifier>;
    type State = ();
    type Signals = Option<Self::State>;
    type Options = NoProcessGlobalOptions;

    fn run(ctx: &RuleContext<Self>) -> Self::Signals {
        let node = ctx.query();
        let model = ctx.model();
        let (reference, name) = global_identifier(node)?;
        if name.text() != "process" {
            return None;
        }
        model.binding(&reference).is_none().then_some(())
    }

    fn diagnostic(ctx: &RuleContext<Self>, _state: &Self::State) -> Option<RuleDiagnostic> {
        let node = ctx.query();
        Some(
            RuleDiagnostic::new(
                rule_category!(),
                node.range(),
                markup! {
                    "Usage of the `process` global is discouraged."
                },
            ).note(markup! {
                "`process` global is hard for tools to statically analyze, so code should not assume they are available."
            })
        )
    }

    fn action(ctx: &RuleContext<Self>, _state: &Self::State) -> Option<JsRuleAction> {
        let node = ctx.query();
        let mut mutation = ctx.root().begin();
        let message = match node {
            AnyPossibleGlobalIdentifier::JsIdentifierExpression(_) => {
                add_process_import(&mut mutation, node, None)?;
                markup! { "Add `import process from \"node:process\";` to this file's imports." }
                    .to_owned()
            }
            // An import doesn't change what `globalThis.process` reads, so the fix also replaces
            // the access with a reference to the imported `process`.
            AnyPossibleGlobalIdentifier::AnyJsMemberExpression(member) => {
                if is_delete_operand(member) || member.syntax().has_inner_comments() {
                    return None;
                }
                let process = AnyJsExpression::from(make::js_identifier_expression(
                    make::js_reference_identifier(make::ident("process")),
                ));
                let declaration = ctx
                    .model()
                    .scope(node.syntax())
                    .ancestors()
                    .find_map(|scope| scope.get_binding("process"));
                match declaration {
                    None => add_process_import(&mut mutation, node, Some(process))?,
                    Some(binding) if is_process_default_import(&binding) => {
                        mutation.replace_node(AnyJsExpression::from(member.clone()), process);
                    }
                    // `process` would refer to this declaration instead of the import.
                    Some(_) => return None,
                }
                markup! { "Use `process` imported from `node:process` instead." }.to_owned()
            }
        };

        Some(JsRuleAction::new(
            ctx.metadata().action_category(ctx.category(), ctx.group()),
            ctx.metadata().applicability(),
            message,
            mutation,
        ))
    }
}

/// Adds `import process from "node:process";` to the module that contains `node`, and puts
/// `replacement` in the place of `node` if there is one.
///
/// Adding the import replaces the whole list of module items, so a replacement of `node` that
/// `mutation` records on its own would be lost.
///
/// Returns `None` if `node` isn't inside a module, because a script can't contain an `import`
/// declaration.
fn add_process_import(
    mutation: &mut BatchMutation<JsLanguage>,
    node: &AnyPossibleGlobalIdentifier,
    replacement: Option<AnyJsExpression>,
) -> Option<()> {
    let node = node.syntax();
    let top_level_statement = node.ancestors().skip(1).find(is_top_level_statement)?;
    // insert new import at:
    // 1. after the most recent import statement. Or, if no such import exist
    // 2. before the first statement, after any leading trivia
    let mut most_recent_import = None;
    let mut sibling_iter = top_level_statement.prev_sibling();
    while let Some(node) = sibling_iter {
        if matches!(node.kind(), JsSyntaxKind::JS_IMPORT) {
            most_recent_import = Some(node);
            break;
        }
        sibling_iter = node.prev_sibling();
    }

    let module_item_list = top_level_statement
        .parent()
        .and_then(JsModuleItemList::cast)?
        .into_syntax();
    let new_module_item_list = match replacement {
        Some(replacement) => {
            // The comments around `node` stay, as they do with `BatchMutation::replace_node`.
            let replacement = replacement
                .with_leading_trivia_pieces(node.first_leading_trivia()?.pieces())?
                .with_trailing_trivia_pieces(node.last_trailing_trivia()?.pieces())?;
            module_item_list
                .clone()
                .replace_child(node.clone().into(), replacement.into_syntax().into())?
        }
        None => module_item_list.clone(),
    };
    let mut slot = 0;
    let new_items: SmallVec<[SyntaxNode<JsLanguage>; 2]>;

    if let Some(import) = most_recent_import {
        slot = import.index();
        new_items = smallvec![import, create_process_import(false).into()];
    } else {
        new_items = smallvec![
            create_process_import(true).into(),
            new_module_item_list
                .first_child()?
                .with_leading_trivia_pieces([])?,
        ];
    }

    let new_module_item_list = new_module_item_list.splice_slots(
        slot..(slot + 1),
        new_items
            .into_iter()
            .map(|item| Some(SyntaxElement::Node(item))),
    );
    mutation.replace_element(module_item_list.into(), new_module_item_list.into());
    Some(())
}

/// Returns `true` if `member` is the operand of `delete`, possibly wrapped in parentheses.
///
/// Replacing the operand with `process` would be a syntax error, because strict mode code, which
/// includes every module, can't delete a variable.
fn is_delete_operand(member: &AnyJsMemberExpression) -> bool {
    member
        .syntax()
        .ancestors()
        .skip(1)
        .find(|ancestor| ancestor.kind() != JsSyntaxKind::JS_PARENTHESIZED_EXPRESSION)
        .and_then(JsUnaryExpression::cast)
        .is_some_and(|unary| unary.operator() == Ok(JsUnaryOperator::Delete))
}

/// Returns `true` if `binding` is the default import of the `process` module as a value, as in
/// `import process from "node:process"`.
fn is_process_default_import(binding: &Binding) -> bool {
    let Some(specifier) = binding
        .syntax()
        .parent()
        .and_then(JsDefaultImportSpecifier::cast)
    else {
        return false;
    };
    let is_type_only = specifier
        .parent::<JsImportDefaultClause>()
        .is_some_and(|clause| clause.type_token().is_some());
    !is_type_only && is_process_module_import(binding)
}

fn is_top_level_statement(node: &SyntaxNode<JsLanguage>) -> bool {
    node.parent()
        .is_some_and(|node| JsModuleItemList::can_cast(node.kind()))
}

fn create_process_import(with_trailing_new_line: bool) -> JsImport {
    let mut semicolon = make::token(T![;]);
    if with_trailing_new_line {
        semicolon = semicolon.with_trailing_trivia([(TriviaPieceKind::Newline, "\n")]);
    }

    let source = make::js_module_source(make::js_string_literal("node:process"));
    let binding = make::js_identifier_binding(make::ident("process").with_trailing_space());
    let specifier = make::js_default_import_specifier(binding.into());
    let clause = make::js_import_default_clause(
        specifier,
        make::token(T![from]).with_trailing_space(),
        source.into(),
    )
    .build();

    make::js_import(
        make::token(T![import])
            .with_trailing_space()
            .with_leading_newline(),
        clause.into(),
    )
    .with_semicolon_token(semicolon)
    .build()
}
