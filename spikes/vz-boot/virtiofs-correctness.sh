busybox mkdir -p /mnt/marsa
busybox mount -t virtiofs marsa-spike /mnt/marsa
cd /mnt/marsa/observations
busybox stat -c 'VF_GUEST stat=%n uid=%u gid=%g mode=%a inode=%i nlink=%h' host-file host-hard
busybox stat -c 'VF_GUEST stat=%n uid=%u gid=%g mode=%a type=%F' host-relative host-dangling
test "$(busybox readlink host-relative)" = host-file
test "$(busybox cat host-relative)" = host-file
test "$(busybox readlink host-dangling)" = absent
test ! -e host-dangling

if test -f mixedcase; then
    printf 'VF_CASE guest_matches_different_case=true\n'
else
    printf 'VF_CASE guest_matches_different_case=false\n'
fi

umask 027
printf 'guest-root\n' > guest-root
busybox chmod 0710 guest-root
busybox ln -s guest-root guest-relative
busybox ln -s absent guest-dangling
if busybox ln guest-root guest-hard; then
    printf 'VF_HARDLINK guest_create=true\n'
else
    printf 'VF_HARDLINK guest_create=false\n'
fi
busybox stat -c 'VF_GUEST stat=%n uid=%u gid=%g mode=%a inode=%i nlink=%h' guest-root
if /mnt/marsa/fsprobe create-unprivileged /mnt/marsa/observations; then
    busybox stat -c 'VF_GUEST stat=%n uid=%u gid=%g mode=%a inode=%i nlink=%h' guest-unprivileged
else
    printf 'VF_GUEST_CREATOR write=false
'
fi
/mnt/marsa/fsprobe xattrs /mnt/marsa/observations
/mnt/marsa/fsprobe watch /mnt/marsa/observations
cd /
busybox umount /mnt/marsa
