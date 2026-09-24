import { describe, expect, it } from 'vitest';
import { render, screen, within } from '@testing-library/svelte';
import Problem from './Problem.svelte';
import { ApiError } from '$lib/api';

const getfd = new ApiError(
	502,
	"libvirt: internal error: unable to execute QEMU command 'getfd': No file descriptor supplied via SCM_RIGHTS",
	{
		cause:
			'AppArmor on this host blocks /dev/vhost-net, so QEMU cannot get the network device for the new NIC.',
		fix: 'Add the rule `/dev/vhost-net rw,` to /etc/apparmor.d/local/abstractions/libvirt-qemu, then stop and start the VM, because a VM gets the rule at its next start.',
		commands: [
			'sudo mkdir -p /etc/apparmor.d/local/abstractions',
			"printf '\\n/dev/vhost-net rw,\\n' | sudo tee -a /etc/apparmor.d/local/abstractions/libvirt-qemu"
		]
	}
);

describe('a problem', () => {
	it('shows the cause, the fix, and the commands of a known libvirt error', () => {
		render(Problem, { props: { error: getfd } });
		const alert = screen.getByRole('alert');
		expect(alert).toHaveTextContent(
			"Libvirt: internal error: unable to execute QEMU command 'getfd'"
		);
		expect(alert).toHaveTextContent('AppArmor on this host blocks /dev/vhost-net');
		expect(alert).toHaveTextContent('Add the rule `/dev/vhost-net rw,`');
		const commands = within(alert).getByLabelText('Commands to run on the host');
		expect(commands.textContent).toBe(getfd.explanation!.commands.join('\n'));
	});

	it('shows an unknown error as its own text only', () => {
		render(Problem, { props: { error: new ApiError(502, 'libvirt: operation failed: new') } });
		const alert = screen.getByRole('alert');
		expect(alert.textContent?.trim()).toBe('Libvirt: operation failed: new.');
		expect(screen.queryByLabelText('Commands to run on the host')).toBeNull();
	});

	it('shows a known error without commands without the command box', () => {
		const snapshot = new ApiError(409, 'libvirt: unsupported configuration: deletion …', {
			cause: 'libvirt cannot delete the current external snapshot while it has a child snapshot.',
			fix: 'Delete its child snapshots first.',
			commands: []
		});
		render(Problem, { props: { error: snapshot } });
		expect(screen.getByRole('alert')).toHaveTextContent('Delete its child snapshots first.');
		expect(screen.queryByLabelText('Commands to run on the host')).toBeNull();
	});
});
