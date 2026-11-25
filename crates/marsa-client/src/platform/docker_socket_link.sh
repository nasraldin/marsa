# Only the Settings client invokes this script with administrator privileges.
# The public link is fixed by the client. Never replace an occupied entry.
action=$1
link=$2
target=$3

if [ -L "$link" ]; then
    current=$(/usr/bin/readlink -n "$link" && /usr/bin/printf x) || exit 1
    if [ "$current" != "${target}x" ]; then
        exit 1
    fi
    case "$action" in
        install) exit 0 ;;
        remove) exec /bin/unlink "$link" ;;
        *) exit 1 ;;
    esac
fi

[ ! -e "$link" ] || exit 1
case "$action" in
    install) exec /bin/ln -sh "$target" "$link" ;;
    remove) exit 0 ;;
    *) exit 1 ;;
esac
