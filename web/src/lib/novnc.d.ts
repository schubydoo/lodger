// Types for the parts of noVNC 1.7.0 that Lodger uses (docs/API.md in the
// package). noVNC ships no types of its own.
declare module '@novnc/novnc' {
	export interface RfbOptions {
		shared?: boolean;
		wsProtocols?: string[];
	}

	export default class RFB extends EventTarget {
		constructor(target: HTMLElement, urlOrChannel: string, options?: RfbOptions);
		scaleViewport: boolean;
		resizeSession: boolean;
		focusOnClick: boolean;
		background: string;
		focus(): void;
		disconnect(): void;
		sendCtrlAltDel(): void;
	}
}
