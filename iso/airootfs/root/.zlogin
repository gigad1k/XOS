# Runs when the live system logs in as root, which is the moment somebody is
# looking at a screen wondering what to do next.
#
# It does not start the installer by itself. Something that begins erasing
# disks because a machine was left booted from a USB stick is a bad thing to
# build, however convenient. It says what to type, in one line, and waits.

if [[ "$(tty)" == "/dev/tty1" ]]; then
    cat /etc/motd
fi
