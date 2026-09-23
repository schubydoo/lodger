// Types for the two SPDX helpers that notices.ts (outside src/) uses. Neither
// package ships its own. The file sits in src/, which the TypeScript project
// includes.

declare module 'spdx-satisfies' {
	/** Whether the license expression `expression` allows one of the `approved` licenses. */
	export default function satisfies(expression: string, approved: string[]): boolean;
}

declare module 'spdx-expression-validate' {
	/** Whether `expression` is a valid SPDX license expression. */
	export default function validate(expression: string): boolean;
}
