# AppArmor

On Debian 13, AppArmor blocks QEMU from opening `/dev/vhost-net` for a network card
that you add to a running VM. The hot-plug then fails. A network card that the VM
has at its start is not affected.

## Add the rule

Add one line to the local AppArmor file for QEMU:

```sh
sudo mkdir -p /etc/apparmor.d/local/abstractions
printf '\n/dev/vhost-net rw,\n' | sudo tee -a /etc/apparmor.d/local/abstractions/libvirt-qemu
```

A VM gets the rule at its next start. Package upgrades keep the file in
`/etc/apparmor.d/local/`.

## Check the rule

```sh
sudo lodger doctor
```

The line `AppArmor vhost rule` must say PASS. On a host without AppArmor for
libvirt, such as Rocky Linux, the check says SKIP.
