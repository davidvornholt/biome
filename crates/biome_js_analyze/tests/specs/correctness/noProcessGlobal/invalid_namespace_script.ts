namespace A {
	export const a = process.env;
}

declare namespace B {
	const b: typeof process;
}

const c = process.cwd();
