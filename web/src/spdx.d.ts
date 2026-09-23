// Types for the two SPDX helpers that notices.ts (outside src/) uses. Neither
// package ships its own. The file sits in src/, which the TypeScript project
// includes.

declare module 'spdx-satisfies' {
	/** Whether the license expression `first` satisfies the expression `second`. */
	export default function satisfies(first: string, second: string): boolean;
}

declare module 'spdx-expression-validate' {
	/** Whether `expression` is a valid SPDX license expression. */
	export default function validate(expression: string): boolean;
}
