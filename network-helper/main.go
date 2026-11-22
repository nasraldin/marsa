// Marsa's user-space guest network runs on an inherited Ethernet socketpair.
package main

import (
	"context"
	"errors"
	"fmt"
	"net"
	"os"
	"os/signal"
	"syscall"
	"time"

	"golang.org/x/sys/unix"
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if err := run(ctx); err != nil {
		fmt.Fprintln(os.Stderr, "marsa-net-helper:", err)
		os.Exit(1)
	}
}

func run(ctx context.Context) error {
	if len(os.Args) != 1 || os.Getuid() == 0 || os.Geteuid() == 0 {
		return errors.New("expected no arguments and an unprivileged user")
	}
	conn, err := inheritedSocket(os.Stdin, unix.SOCK_DGRAM, "stdin must be a connected Unix datagram socket")
	if err != nil {
		return err
	}
	defer conn.Close()

	control, err := inheritedSocket(os.Stdout, unix.SOCK_STREAM, "stdout must be a connected Unix stream socket")
	if err != nil {
		return err
	}
	defer control.Close()

	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	go func() {
		// A daemon never sends bytes. EOF also arrives when the daemon is killed
		// and cannot run its normal child cleanup.
		var unexpected [1]byte
		_, _ = control.Read(unexpected[:])
		cancel()
	}()

	// The only host-side Ethernet transport is this connected descriptor.
	switcher, shutdown, err := startNetwork(ctx)
	if err != nil {
		return err
	}
	defer shutdown()

	if _, err := fmt.Fprintln(control, "READY"); err != nil {
		return err
	}
	result := make(chan error, 1)
	go func() {
		result <- switcher(ctx, &ethernetConn{UnixConn: conn})
	}()
	select {
	case <-ctx.Done():
		return nil
	case <-result:
		return errors.New("guest datagram transport closed")
	}
}

func inheritedSocket(input *os.File, socketType int, description string) (*net.UnixConn, error) {
	fd := int(input.Fd())
	kind, err := unix.GetsockoptInt(fd, unix.SOL_SOCKET, unix.SO_TYPE)
	if err != nil || kind != socketType {
		return nil, errors.New(description)
	}
	peer, err := unix.Getpeername(fd)
	if _, isUnix := peer.(*unix.SockaddrUnix); err != nil || !isUnix {
		return nil, errors.New(description)
	}
	duplicate, err := net.FileConn(input)
	if err != nil {
		return nil, fmt.Errorf("duplicate inherited socket: %w", err)
	}
	conn, ok := duplicate.(*net.UnixConn)
	if !ok {
		duplicate.Close()
		return nil, errors.New(description)
	}
	return conn, nil
}

const (
	headerBytes = 14
	mtu         = 1500
)

// Both endpoints use this MAC in the isolated VM's file-handle attachment.
var guestMAC = [6]byte{0x5a, 0x94, 0xef, 0xe4, 0x0c, 0xee}

type ethernetConn struct{ *net.UnixConn }

func (c *ethernetConn) Read(frame []byte) (int, error) {
	for {
		n, err := c.UnixConn.Read(frame)
		if err != nil {
			return 0, err
		}
		if n >= headerBytes && n <= mtu+headerBytes &&
			[6]byte(frame[6:12]) == guestMAC &&
			(frame[12] == 0x08 && (frame[13] == 0x00 || frame[13] == 0x06)) {
			return n, nil
		}
	}
}

func (c *ethernetConn) Write(frame []byte) (int, error) {
	if err := c.SetWriteDeadline(time.Now().Add(time.Second)); err != nil {
		return 0, err
	}
	n, err := c.UnixConn.Write(frame)
	if errors.Is(err, unix.ENOBUFS) {
		// Drop this frame. The upstream switch retries ENOBUFS without a deadline.
		return len(frame), nil
	}
	return n, err
}
