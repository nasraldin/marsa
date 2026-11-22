package main

import (
	"bytes"
	"crypto/ed25519"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"time"
	"unsafe"
)

const peerPidfd = 77 // Pinned Linux v7.3.5 include/uapi/asm-generic/socket.h.
const socket = "/tmp/marsa-peer.sock"
const port = 5050
const hostCID = 2
const anyCID = ^uint32(0)
const maxFrame = 32 << 10

var challenge = []byte("marsa-vz-ssh-agent-fixture")

type pollFd struct {
	fd     int32
	events int16
	result int16
}

func exited(pidfd int) (bool, error) {
	probe := pollFd{fd: int32(pidfd), events: 1}
	ts := syscall.Timespec{}
	n, _, problem := syscall.Syscall6(syscall.SYS_PPOLL, uintptr(unsafe.Pointer(&probe)), 1, uintptr(unsafe.Pointer(&ts)), 0, 0, 0)
	if problem != 0 {
		return false, problem
	}
	return n > 0, nil
}

func peer(conn *net.UnixConn) (string, int, error) {
	raw, err := conn.SyscallConn()
	if err != nil {
		return "", -1, err
	}
	var peer *syscall.Ucred
	var pidfd = -1
	if e := raw.Control(func(fd uintptr) {
		peer, err = syscall.GetsockoptUcred(int(fd), syscall.SOL_SOCKET, syscall.SO_PEERCRED)
		if err == nil {
			pidfd, err = syscall.GetsockoptInt(int(fd), syscall.SOL_SOCKET, peerPidfd)
		}
	}); e != nil {
		return "", -1, e
	}
	if err != nil {
		return "", -1, err
	}
	quit, err := exited(pidfd)
	if err != nil || quit {
		syscall.Close(pidfd)
		return "", -1, fmt.Errorf("peer already exited or cannot be checked: %v", err)
	}
	cgroup, err := os.ReadFile(fmt.Sprintf("/proc/%d/cgroup", peer.Pid))
	quit, check := exited(pidfd)
	if err != nil || check != nil || quit {
		syscall.Close(pidfd)
		return "", -1, fmt.Errorf("peer exited or cannot be classified: %v / %v", err, check)
	}
	return string(cgroup), pidfd, nil
}

func check(e error) {
	if e != nil {
		panic(e)
	}
}

// sockaddr_vm follows Linux v7.3.5's vm_sockets.h layout on arm64.
type vmAddress struct {
	family   uint16
	reserved uint16
	port     uint32
	cid      uint32
	flags    uint8
	pad      [3]byte
}

func vmSocket() (int, error) {
	return syscall.Socket(syscall.AF_VSOCK, syscall.SOCK_STREAM|syscall.SOCK_CLOEXEC, 0)
}

func vmSockAddr(fd int, call uintptr, addr *vmAddress) error {
	_, _, problem := syscall.Syscall(call, uintptr(fd), uintptr(unsafe.Pointer(addr)), unsafe.Sizeof(*addr))
	if problem != 0 {
		return problem
	}
	return nil
}

func inactiveHostRejectsUnsolicitedDial() {
	fd, err := vmSocket()
	check(err)
	defer syscall.Close(fd)
	check(syscall.SetNonblock(fd, true))
	addr := vmAddress{family: syscall.AF_VSOCK, cid: hostCID, port: port}
	err = vmSockAddr(fd, syscall.SYS_CONNECT, &addr)
	if err == nil {
		panic("unexpected unrequested guest-to-host transport")
	}
	if errors.Is(err, syscall.EINPROGRESS) {
		probe := pollFd{fd: int32(fd), events: 4}
		ts := syscall.Timespec{Sec: 1}
		n, _, problem := syscall.Syscall6(syscall.SYS_PPOLL, uintptr(unsafe.Pointer(&probe)), 1, uintptr(unsafe.Pointer(&ts)), 0, 0, 0)
		if problem != 0 {
			panic(problem)
		}
		if n != 0 {
			socketError, err := syscall.GetsockoptInt(fd, syscall.SOL_SOCKET, syscall.SO_ERROR)
			check(err)
			if socketError == 0 {
				panic("unexpected unrequested guest-to-host transport")
			}
		}
	}
	fmt.Println("ssh-fixture: unsolicited guest-to-host connection denied")
}

func acceptHost() *os.File {
	listener, err := vmSocket()
	check(err)
	defer syscall.Close(listener)
	addr := vmAddress{family: syscall.AF_VSOCK, port: port, cid: anyCID}
	check(vmSockAddr(listener, syscall.SYS_BIND, &addr))
	check(syscall.Listen(listener, 1))
	// The Go syscall Sockaddr wrapper has no AF_VSOCK decoder. Accept with
	// null address outputs, then get the fixed-size peer from Linux.
	for {
		accepted, _, problem := syscall.Syscall6(syscall.SYS_ACCEPT4, uintptr(listener), 0, 0, syscall.SOCK_CLOEXEC, 0, 0)
		if problem != 0 {
			panic(problem)
		}
		size := uint32(unsafe.Sizeof(addr))
		var peer vmAddress
		_, _, problem = syscall.Syscall(syscall.SYS_GETPEERNAME, accepted, uintptr(unsafe.Pointer(&peer)), uintptr(unsafe.Pointer(&size)))
		if problem == 0 && size == uint32(unsafe.Sizeof(peer)) && peer.family == syscall.AF_VSOCK && peer.cid == hostCID {
			return os.NewFile(accepted, "host-vsock")
		}
		syscall.Close(int(accepted))
	}
}

func readFrame(input io.Reader) []byte {
	var prefix [4]byte
	_, err := io.ReadFull(input, prefix[:])
	check(err)
	size := binary.BigEndian.Uint32(prefix[:])
	if size == 0 || size > maxFrame {
		panic("invalid fixture SSH packet length")
	}
	frame := make([]byte, size)
	_, err = io.ReadFull(input, frame)
	check(err)
	return frame
}

func uint32Bytes(value uint32) []byte {
	return binary.BigEndian.AppendUint32(nil, value)
}

func writeFrame(output io.Writer, payload []byte) {
	if len(payload) == 0 || len(payload) > maxFrame {
		panic("invalid fixture SSH packet length")
	}
	_, err := output.Write(append(uint32Bytes(uint32(len(payload))), payload...))
	check(err)
}

func sshString(value []byte) []byte {
	return append(uint32Bytes(uint32(len(value))), value...)
}

func takeString(input []byte) ([]byte, []byte) {
	if len(input) < 4 {
		panic("invalid fixture SSH string")
	}
	n := int(binary.BigEndian.Uint32(input[:4]))
	if n > len(input)-4 {
		panic("invalid fixture SSH string")
	}
	return input[4 : 4+n], input[4+n:]
}

func runClient(approved string) {
	var signal [1]byte
	_, err := io.ReadFull(os.Stdin, signal[:])
	check(err)
	conn, err := net.DialUnix("unix", nil, &net.UnixAddr{Name: socket, Net: "unix"})
	check(err)
	defer conn.Close()
	check(conn.SetDeadline(time.Now().Add(12 * time.Second)))
	if approved == "denied" {
		_, err = conn.Read(signal[:])
		if !errors.Is(err, io.EOF) {
			panic("unapproved same-socket client did not receive EOF")
		}
		return
	}

	writeFrame(conn, []byte{11})
	identities := readFrame(conn)
	if len(identities) < 5 || !bytes.Equal(identities[:5], []byte{12, 0, 0, 0, 1}) {
		panic("expected exactly one disposable public identity")
	}
	key, tail := takeString(identities[5:])
	_, extras := takeString(tail) // comment
	kind, public := takeString(key)
	signingKey, remainder := takeString(public)
	if !bytes.Equal(kind, []byte("ssh-ed25519")) || len(signingKey) != ed25519.PublicKeySize || len(remainder)+len(extras) != 0 {
		panic("unexpected fixture agent identity")
	}

	writeFrame(conn, []byte{19})
	if !bytes.Equal(readFrame(conn), []byte{5}) {
		panic("agent mutation must be refused")
	}
	request := append([]byte{13}, sshString(key)...)
	request = append(request, sshString(challenge)...)
	request = append(request, uint32Bytes(0)...)
	writeFrame(conn, request)
	answer := readFrame(conn)
	if answer[0] != 14 {
		panic("expected signature response")
	}
	blob, tail := takeString(answer[1:])
	kind, blob = takeString(blob)
	signature, remainder := takeString(blob)
	if !bytes.Equal(kind, []byte("ssh-ed25519")) || len(tail)+len(remainder) != 0 || !ed25519.Verify(signingKey, challenge, signature) {
		panic("disposable agent signature was invalid")
	}
	fmt.Println("ssh-fixture: approved opener verified signature")
	_, err = conn.Read(signal[:])
	if !errors.Is(err, io.EOF) {
		panic("active peer did not observe revocation")
	}
}

func startClient(label, mode string) *exec.Cmd {
	cgroup := filepath.Join("/sys/fs/cgroup/docker", label)
	check(os.MkdirAll(cgroup, 0755))
	cmd := exec.Command(os.Args[0], mode)
	start, err := cmd.StdinPipe()
	check(err)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	check(cmd.Start())
	check(os.WriteFile(filepath.Join(cgroup, "cgroup.procs"), []byte(fmt.Sprint(cmd.Process.Pid)), 0644))
	_, err = start.Write([]byte{1})
	check(err)
	return cmd
}

func main() {
	if len(os.Args) == 2 {
		runClient(os.Args[1])
		return
	}
	inactiveHostRejectsUnsolicitedDial()
	host := acceptHost()
	defer host.Close()
	_, err := host.Write([]byte("H"))
	check(err)
	var signal [1]byte
	_, err = io.ReadFull(host, signal[:])
	check(err)
	if signal[0] != 'G' {
		panic("no host fixture grant")
	}

	listener, err := net.ListenUnix("unix", &net.UnixAddr{Name: socket, Net: "unix"})
	check(err)
	defer listener.Close()
	check(listener.SetDeadline(time.Now().Add(10 * time.Second)))
	approved := strings.Repeat("a", 64)
	denied := startClient(strings.Repeat("b", 64), "denied")
	other, err := listener.AcceptUnix()
	check(err)
	actual, pidfd, err := peer(other)
	check(err)
	if actual != "0::/docker/"+strings.Repeat("b", 64)+"\n" || actual == "0::/docker/"+approved+"\n" {
		panic("same-socket opener must retain its own cgroup identity")
	}
	syscall.Close(pidfd)
	check(other.Close())
	check(denied.Wait())
	fmt.Println("ssh-fixture: denied opener from other cgroup")

	selected := startClient(approved, "approved")
	client, err := listener.AcceptUnix()
	check(err)
	defer client.Close()
	actual, pidfd, err = peer(client)
	check(err)
	defer syscall.Close(pidfd)
	if actual != "0::/docker/"+approved+"\n" {
		panic("unexpected cgroup for approved peer")
	}
	check(client.SetDeadline(time.Now().Add(12 * time.Second)))
	for range 3 {
		writeFrame(host, readFrame(client))
		writeFrame(client, readFrame(host))
	}

	_, err = host.Read(signal[:])
	// Darwin's VZ proxy may report reset instead of EOF when the host drops
	// the native connection. Either terminates the fixture authorization.
	if !errors.Is(err, io.EOF) && !errors.Is(err, syscall.ECONNRESET) {
		panic("host did not close the fixture transport")
	}
	check(listener.Close())
	check(client.Close())
	check(selected.Wait())
	dead, err := exited(pidfd)
	check(err)
	if !dead {
		panic("peer pidfd did not signal exit")
	}
	fmt.Println("ssh-fixture: approved peer pidfd signaled exit")
	unexpected, err := net.DialUnix("unix", nil, &net.UnixAddr{Name: socket, Net: "unix"})
	if err == nil {
		unexpected.Close()
		panic("fresh connection survived revocation")
	}
	fmt.Println("ssh-fixture: revocation cut active and fresh streams")
}
