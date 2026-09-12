# Runs when the live system logs itself in on the first console.
#
# The medium boots straight into the installer: somebody who put a stick in a
# machine to install XOS should not then have to be told a command to type.
#
# What it does not do is erase anything without being asked. The installer
# still shows the disk and waits for its name to be typed back. The boot menu
# has a separate entry for an unattended install, and that entry says so.
#
# Ctrl+C leaves a shell, so nobody is trapped in an installer they opened by
# accident.

if [[ "$(tty)" == "/dev/tty1" ]]; then
    cat /etc/motd

    # The boot menu carries the choice, so the kernel command line is where
    # this reads it from.
    if grep -qw 'xos.auto' /proc/cmdline; then
        exec install-xos --auto
    fi

    install-xos
    echo
    echo "  The installer has exited. Type install-xos to start it again."
    echo
fi
