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
#
# # Once per boot, and never exec
#
# agetty respawns the login when its shell exits. This file is that shell, so
# starting the installer here and letting it be the whole session means that an
# installer which exits for any reason is started again immediately, forever.
# On the unattended entry that is not a cosmetic loop: it is a machine retrying
# an erase every couple of minutes with nobody watching.
#
# So two rules. A marker in /run, which is tmpfs and therefore empty on every
# boot, means the installer runs once per boot and a second login says so
# instead of restarting it. And never exec: the shell has to outlive the
# installer, or a failed install leaves no way to read the log that would say
# why — which is exactly how this was found.

if [[ "$(tty)" == "/dev/tty1" ]]; then
    cat /etc/motd

    if [[ -e /run/xos-installer-started ]]; then
        echo
        echo "  The installer already ran during this boot, so it has not been"
        echo "  started again. Type install-xos to run it, or reboot to start over."
        echo
    else
        : > /run/xos-installer-started

        # The boot menu carries the choice, so the kernel command line is where
        # this reads it from.
        if grep -qw 'xos.auto' /proc/cmdline; then
            install-xos --auto
        else
            install-xos
        fi

        echo
        echo "  The installer has exited. Type install-xos to start it again."
        echo
    fi
fi
