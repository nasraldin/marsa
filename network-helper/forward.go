package main

import (
	"context"
	"io"
	"net"
	"net/netip"
	"strconv"
	"time"

	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/adapters/gonet"
	"gvisor.dev/gvisor/pkg/tcpip/stack"
	"gvisor.dev/gvisor/pkg/tcpip/transport/tcp"
	"gvisor.dev/gvisor/pkg/tcpip/transport/udp"
	"gvisor.dev/gvisor/pkg/waiter"
)

var blockedDestinations = []netip.Prefix{
	netip.MustParsePrefix("0.0.0.0/8"),          // local network and unspecified
	netip.MustParsePrefix("127.0.0.0/8"),        // loopback aliases
	netip.MustParsePrefix("169.254.0.0/16"),     // link-local and cloud metadata
	netip.MustParsePrefix("100.100.100.200/32"), // Alibaba metadata
	netip.MustParsePrefix("168.63.129.16/32"),   // Azure platform endpoint
	netip.MustParsePrefix("192.0.0.192/32"),     // Oracle metadata
	netip.MustParsePrefix("224.0.0.0/3"),        // multicast and reserved/broadcast
	guestSubnet,
}

func permitted(address netip.Addr, port uint16, local func() ([]net.Addr, error)) bool {
	// Guest DNS must go through the virtual gateway's scoped resolver.
	if !address.Is4() || port == 0 || port == 53 {
		return false
	}
	for _, blocked := range blockedDestinations {
		if blocked.Contains(address) {
			return false
		}
	}
	// Read current addresses on each new flow, including VPN interface addresses.
	bindings, err := local()
	if err != nil {
		return false
	}
	for _, binding := range bindings {
		prefix, err := netip.ParsePrefix(binding.String())
		if err != nil || prefix.Addr().Unmap() == address {
			return false
		}
	}
	return true
}

const maxHostFlows = 128

func startForwarding(ctx context.Context, s *stack.Stack) {
	slots := make(chan struct{}, maxHostFlows)
	reserve := func() bool {
		select {
		case slots <- struct{}{}:
			return true
		default:
			return false
		}
	}

	dial := func(protocol string, id stack.TransportEndpointID) net.Conn {
		if id.LocalAddress.Len() != 4 || id.RemoteAddress != tcpip.AddrFrom4([4]byte{192, 168, 127, 2}) {
			return nil
		}
		address := netip.AddrFrom4(id.LocalAddress.As4())
		if !permitted(address, id.LocalPort, net.InterfaceAddrs) {
			return nil
		}
		dialer := net.Dialer{Timeout: 8 * time.Second}
		out, err := dialer.DialContext(ctx, protocol, net.JoinHostPort(address.String(), strconv.Itoa(int(id.LocalPort))))
		if err != nil {
			return nil
		}
		return out
	}

	tcpForwarder := tcp.NewForwarder(s, 0, maxHostFlows, func(r *tcp.ForwarderRequest) {
		if !reserve() {
			r.Complete(true)
			return
		}
		defer func() { <-slots }()
		host := dial("tcp4", r.ID())
		if host == nil {
			r.Complete(true)
			return
		}
		defer host.Close()

		var waiterQueue waiter.Queue
		endpoint, err := r.CreateEndpoint(&waiterQueue)
		r.Complete(false)
		if err != nil {
			return
		}
		guest := gonet.NewTCPConn(&waiterQueue, endpoint)
		defer guest.Close()
		relayTCP(guest, host.(*net.TCPConn))
	})
	s.SetTransportProtocolHandler(tcp.ProtocolNumber, tcpForwarder.HandlePacket)

	udpForwarder := udp.NewForwarder(s, func(r *udp.ForwarderRequest) bool {
		if !reserve() {
			return true
		}
		host := dial("udp4", r.ID())
		if host == nil {
			<-slots
			return true
		}
		var waiterQueue waiter.Queue
		endpoint, err := r.CreateEndpoint(&waiterQueue)
		if err != nil {
			host.Close()
			<-slots
			return true
		}
		guest := gonet.NewUDPConn(&waiterQueue, endpoint)
		go func() {
			defer func() { <-slots }()
			relayUDP(guest, host)
		}()
		return true
	})
	s.SetTransportProtocolHandler(udp.ProtocolNumber, udpForwarder.HandlePacket)
}

type halfCloseConn interface {
	net.Conn
	CloseWrite() error
}

func relayTCP(guest, host halfCloseConn) {
	done := make(chan struct{})
	copyStream := func(dst, src halfCloseConn) {
		if _, err := io.Copy(dst, src); err != nil {
			dst.Close()
			src.Close()
		}
		dst.CloseWrite()
	}
	go func() {
		copyStream(guest, host)
		close(done)
	}()
	copyStream(host, guest)
	<-done
}

func relayUDP(guest, host net.Conn) {
	done := make(chan struct{}, 2)
	copyDatagrams := func(dst, src net.Conn) {
		defer func() { done <- struct{}{} }()
		buffer := make([]byte, 65535)
		for {
			if err := src.SetReadDeadline(time.Now().Add(30 * time.Second)); err != nil {
				return
			}
			n, err := src.Read(buffer)
			if err != nil {
				return
			}
			if err := dst.SetWriteDeadline(time.Now().Add(time.Second)); err != nil {
				return
			}
			if _, err := dst.Write(buffer[:n]); err != nil {
				return
			}
		}
	}
	go copyDatagrams(guest, host)
	go copyDatagrams(host, guest)
	<-done
	guest.Close()
	host.Close()
	<-done
}
