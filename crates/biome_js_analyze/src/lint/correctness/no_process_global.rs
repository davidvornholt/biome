use crate::JsRuleAction;
use crate::services::semantic::Semantic;
use biome_analyze::{
    FixKind, Rule, RuleDiagnostic, RuleSource, context::RuleContext, declare_lint_rule,
};
use biome_console::markup;
use biome_js_factory::make;
use biome_js_syntax::JsImport;
use biome_js_syntax::JsLanguage;
use biome_js_syntax::JsSyntaxKind;
use biome_js_syntax::T;
use biome_js_syntax::{AnyPossibleGlobalIdentifier, global_identifier};
use biome_rowan::AstNode;
use biome_rowan::BatchMutationExt;
use biome_rowan::SyntaxElement;
use biome_rowan::SyntaxNode;
use biome_rowan::SyntaxTriviaPiece;
use biome_rowan::TriviaPieceKind;
use biome_rule_options::no_process_global::NoProcessGlobalOptions;
use biome_suppression::{SuppressionKind, parse_suppression_comment};
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
        // The import must go into the file's own item list: a TypeScript namespace body is also
        // a `JsModuleItemList`, but it can't contain an import of a module.
        let module_item_list = ctx.root().as_js_module()?.items().into_syntax();
        let top_level_statement = ctx
            .query()
            .syntax()
            .ancestors()
            .skip(1)
            .find(|node| node.parent().as_ref() == Some(&module_item_list))?;
        // In a file without imports or exports, a namespace is global. The new import would turn
        // the file into a module and make the namespace local to it.
        let is_module = module_item_list
            .children()
            .any(|item| matches!(item.kind(), JsSyntaxKind::JS_IMPORT | JsSyntaxKind::JS_EXPORT));
        if !is_module && module_item_list.children().any(|item| declares_namespace(&item)) {
            return None;
        }
        let mut most_recent_import = None;
        let mut sibling_iter = top_level_statement.prev_sibling();
        while let Some(node) = sibling_iter {
            if matches!(node.kind(), JsSyntaxKind::JS_IMPORT) {
                most_recent_import = Some(node);
                break;
            }
            sibling_iter = node.prev_sibling();
        }

        let mut slot = 0;
        let new_items: SmallVec<[SyntaxNode<JsLanguage>; 2]>;

        if let Some(import) = most_recent_import {
            slot = import.index();
            new_items = smallvec![import, create_process_import(false).into()];
        } else {
            let first_item = module_item_list.first_child()?;
            let mut header: Vec<_> = first_item.first_leading_trivia()?.pieces().collect();
            let statement_trivia = header.split_off(statement_trivia_start(&header)?);
            new_items = smallvec![
                create_process_import(true)
                    .into_syntax()
                    .with_leading_trivia_pieces(header)?,
                first_item.with_leading_trivia_pieces(statement_trivia)?,
            ];
        }

        let new_module_item_list = module_item_list.clone().splice_slots(
            slot..(slot + 1),
            new_items
                .into_iter()
                .map(|item| Some(SyntaxElement::Node(item))),
        );

        let mut mutation = ctx.root().begin();
        mutation.replace_element_discard_trivia(
            module_item_list.into(),
            new_module_item_list.into(),
        );

        Some(JsRuleAction::new(
            ctx.metadata().action_category(ctx.category(), ctx.group()),
            ctx.metadata().applicability(),
            markup! { "Add `import process from \"node:process\";` to this file's imports." }
                .to_owned(),
            mutation,
        ))
    }
}

/// Returns whether the module item declares a namespace, such as `namespace A {}`,
/// `declare namespace A {}` or `declare module "a" {}`.
fn declares_namespace(item: &SyntaxNode<JsLanguage>) -> bool {
    let declaration = if item.kind() == JsSyntaxKind::TS_DECLARE_STATEMENT {
        item.last_child()
    } else {
        Some(item.clone())
    };
    declaration.is_some_and(|declaration| {
        matches!(
            declaration.kind(),
            JsSyntaxKind::TS_MODULE_DECLARATION | JsSyntaxKind::TS_EXTERNAL_MODULE_DECLARATION
        )
    })
}

/// Returns the index in `trivia`, the leading trivia of the file's first statement, where the
/// trivia that belongs to the statement starts.
///
/// A `biome-ignore` comment applies to the statement that follows it, so the trivia from the first
/// such comment on, with the indentation before it, stays with the statement. The trivia before it
/// stays above the new import, so that a file header remains at the top.
///
/// Returns `None` when a `biome-ignore-all` comment would end up below the new import, because it
/// only applies at the top of the file.
fn statement_trivia_start(trivia: &[SyntaxTriviaPiece<JsLanguage>]) -> Option<usize> {
    let has_suppression = |piece: &SyntaxTriviaPiece<JsLanguage>, kind: SuppressionKind| {
        piece.kind().is_comment()
            && parse_suppression_comment(piece.text())
                .any(|suppression| suppression.is_ok_and(|s| s.kind == kind))
    };
    let Some(mut start) = trivia
        .iter()
        .position(|piece| has_suppression(piece, SuppressionKind::Classic))
    else {
        return Some(trivia.len());
    };
    if trivia[start..]
        .iter()
        .any(|piece| has_suppression(piece, SuppressionKind::All))
    {
        return None;
    }
    while start > 0 && trivia[start - 1].kind().is_whitespace() {
        start -= 1;
    }
    Some(start)
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
