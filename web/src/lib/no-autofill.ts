/**
 * Attributes for a text field that is not a login field, such as a pool name or
 * a subnet. Password managers ignore `autocomplete="off"`, so each one also gets
 * the attribute that it reads: 1Password, LastPass, Bitwarden, and Dashlane.
 */
export const noAutofill = {
	autocomplete: 'off',
	'data-1p-ignore': '',
	'data-lpignore': 'true',
	'data-bwignore': '',
	'data-form-type': 'other'
} as const;
