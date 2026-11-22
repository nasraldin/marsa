# M9-04 VZ memory measurement

The test ran on an Apple M1 Max with 64 GiB RAM, macOS 27.2.1 (26B131),
pinned Linux 7.3.5 and Docker Engine 29.10.0. It used the signed packaged
daemon with user-space networking. Each case started a fresh VM from the
same guest image in a private temporary home, with a 4 GiB allocation,
a 1 GiB sparse disk and a four-second idle timeout.

The fixture held exactly 2,147,483,648 random bytes in a guest tmpfs. The
container had no network, a 2,600 MiB memory limit and a 2,200 MiB tmpfs
limit. It confirmed the file length while the pages remained allocated,
then stopped the container so the guest released the tmpfs. Each case ran
separately. A read-only preflight required at least 16 GiB host RAM and
30% memory free according to `memory_pressure -Q`.

The measurement read the host's `proc_pid_rusage` physical-footprint ledger
(`ri_phys_footprint`) for the daemon, its user-space helper and the VZ
worker created during that boot. It recorded Linux's reported memory
capacity from the agent and sampled cumulative inflate/deflate counters
before the workload and after renewing Docker use. The VZ target API
requests a change asynchronously. Linux can retain its reported
`MemTotal` when the driver supports deflation on OOM, so the kernel
event counters and the macOS footprint are independent measurements.
[Apple target API](https://developer.apple.com/documentation/virtualization/vzvirtiotraditionalmemoryballoondevice/targetvirtualmachinememorysize)
· [Linux 7.3.5 virtio-balloon driver](https://github.com/gregkh/linux/blob/v7.3.5/drivers/virtio/virtio_balloon.c#L949-L958).

## Results

All figures are bytes of macOS physical footprint. Before and held samples
are relative to starting the workload. Later samples are relative to
completion of the Docker stop request.

| Mode | Sample | Elapsed | VZ worker | Network helper | Daemon |
|---|---|---:|---:|---:|---:|
| No balloon | Before | 0 ms | 1,485,310,544 | 13,845,128 | 19,776,208 |
| No balloon | 2 GiB held | 4,952 ms | 2,585,202,000 | 13,845,128 | 19,661,544 |
| No balloon | After | 25,003 ms | 2,585,202,000 | 13,845,128 | 14,484,200 |
| Balloon to 1.5 GiB | Before | 0 ms | 1,489,111,632 | 13,288,096 | 22,987,520 |
| Balloon to 1.5 GiB | 2 GiB held | 4,746 ms | 2,597,735,784 | 13,288,096 | 22,856,448 |
| Balloon to 1.5 GiB | After | 25,004 ms | 2,597,932,392 | 13,288,096 | 22,971,136 |
| Idle stop | Before | 2 ms | 1,485,228,624 | 13,402,808 | 23,036,672 |
| Idle stop | 2 GiB held | 4,745 ms | 2,587,168,080 | 13,402,808 | 22,823,680 |
| Idle stop | After | 4,387 ms | Exited | Exited | 22,921,984 |

The balloon case requested 1,610,612,736 bytes on entry to Idle at
4,049 ms and restored the boot target before the next Docker call. The
guest recorded 655,360 inflate events and the same number of deflate
events, equivalent to 2.5 GiB in 4 KiB pages in each direction. The
control recorded zero in both directions. The agent reported
4,135,063,552 guest bytes throughout. The VZ footprint held steady
without the balloon and was 196,608 bytes higher than the held sample
with the balloon. Guest inflation therefore produced no positive host
footprint reduction in this 25-second window.

Idle stop reached Stopped at 4,292 ms after the workload ended. By
4,387 ms both VM-related processes had exited and the remaining footprint
belonged to the idle daemon. Its cold baseline was 6,537,648 bytes and
its warm idle footprint was 22,921,984 bytes, about 15.6 MiB above cold.

The A/B run used a temporary measurement build to exercise the VZ
backend's existing memory target API in the signed packaged guest. The
shipping supervisor retains the existing automatic boot sizing and
configurable idle stop. In the tested configuration idle stop released
the VM's footprint. Ballooning stays out of the shipping idle policy.

A separate run from the shipping source repeated the idle-stop result.
The VM reached Stopped in 4,327 ms. Both worker and helper had exited by
4,417 ms, leaving 19,759,824 bytes charged to the idle daemon from a cold
baseline of 6,570,416 bytes. The test allows up to 32 MiB of daemon
warm-up while requiring both runtime processes to exit.

## Reproduce

To repeat the shipping idle-stop measurement, build the guest and its
user-space network helper as for the
[packaged runtime system test](../03-host-daemon.md#docker-socket-proxy).
Set `MARSA_VZ_GUEST_DIR` and `MARSA_VZ_NETWORK_HELPER` to those artifacts.
Set `MARSA_TEST_DOCKER` to the bundled Docker CLI when Docker is unavailable
on `PATH`.

```sh
MACOSX_DEPLOYMENT_TARGET=27.0 cargo test --locked -p marsad \
  --test packaged_runtime packaged_idle_stop_releases_host_memory \
  -- --ignored --nocapture --test-threads=1
```
