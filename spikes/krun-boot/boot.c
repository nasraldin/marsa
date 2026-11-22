/* Qualify stable libkrun devices with a private share and disconnected network. */
#include <libkrun.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <unistd.h>

static void check(const char *operation, int32_t result) {
    if (result < 0) {
        fprintf(stderr, "%s failed with error %d\n", operation, result);
        exit(EXIT_FAILURE);
    }
}

int main(int argc, char **argv) {
    if (argc != 7 || geteuid() == 0) {
        fputs("Usage as a normal user: krun-boot IMAGE ROOTFS DATA CONSOLE CONTROL_SOCKET EMPTY_SHARE\n",
              stderr);
        return EXIT_FAILURE;
    }

    const char *const no_environment[] = {NULL};
    const char *command_line =
        "console=hvc0 root=/dev/vda rootfstype=erofs ro init=/usr/bin/marsa-init "
        "marsa.interface=eth0 marsa.share=fixture:/tmp/krun-check panic=-1";

    check("krun_set_log_level", krun_set_log_level(3));
    int32_t result = krun_create_ctx();
    check("krun_create_ctx", result);
    uint32_t context = (uint32_t)result;

    check("krun_set_vm_config", krun_set_vm_config(context, 2, 1024));
    check("krun_disable_implicit_init", krun_disable_implicit_init(context));
    check("krun_set_env", krun_set_env(context, no_environment));
    check("krun_set_kernel",
          krun_set_kernel(context, argv[1], KRUN_KERNEL_FORMAT_RAW, NULL, command_line));
    check("attach read-only root",
          krun_add_disk3(context, "root", argv[2], KRUN_DISK_FORMAT_RAW, true, false,
                        KRUN_SYNC_FULL));
    check("attach writable data",
          krun_add_disk3(context, "data", argv[3], KRUN_DISK_FORMAT_RAW, false, false,
                        KRUN_SYNC_FULL));
    check("krun_set_console_output", krun_set_console_output(context, argv[4]));
    check("krun_disable_implicit_vsock", krun_disable_implicit_vsock(context));
    check("krun_add_vsock without TSI", krun_add_vsock(context, 0));
    check("guest control socket", krun_add_vsock_port2(context, 5000, argv[5], true));
    check("attach read-only fixture",
          krun_add_virtiofs3(context, "fixture", argv[6], 0, true));

    int network[2];
    if (socketpair(AF_UNIX, SOCK_DGRAM, 0, network) < 0) {
        perror("create isolated network socket pair");
        return EXIT_FAILURE;
    }
    uint8_t mac[] = {0x02, 0x00, 0x00, 0x00, 0x00, 0x19};
    // The second end stays in this process with no forwarding to the host.
    check("attach isolated Ethernet",
          krun_add_net_unixgram(context, NULL, network[0], mac, 0, 0));
    check("krun_start_enter", krun_start_enter(context));
    close(network[1]);
    return EXIT_SUCCESS;
}
