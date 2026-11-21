package main

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/netip"

	"github.com/containers/gvisor-tap-vsock/pkg/services/dhcp"
	"github.com/containers/gvisor-tap-vsock/pkg/tap"
	"github.com/containers/gvisor-tap-vsock/pkg/types"
	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/adapters/gonet"
	"gvisor.dev/gvisor/pkg/tcpip/network/arp"
	"gvisor.dev/gvisor/pkg/tcpip/network/ipv4"
	"gvisor.dev/gvisor/pkg/tcpip/stack"
	"gvisor.dev/gvisor/pkg/tcpip/transport/tcp"
	"gvisor.dev/gvisor/pkg/tcpip/transport/udp"
)

const gatewayMAC = "5a:94:ef:e4:0c:dd"

var guestSubnet = netip.MustParsePrefix("192.168.127.0/24")
var gateway = tcpip.AddrFrom4([4]byte{192, 168, 127, 1})

func startNetwork(ctx context.Context) (func(context.Context, net.Conn) error, func(), error) {
	cfg := &types.Configuration{
		MTU: mtu, Subnet: guestSubnet.String(), GatewayIP: gateway.String(),
		GatewayMacAddress: gatewayMAC, DHCPStaticLeases: map[string]string{
			"192.168.127.2": net.HardwareAddr(guestMAC[:]).String(),
		},
	}

	pool, err := tap.NewIPPool(cfg.Subnet)
	if err != nil {
		return nil, nil, err
	}
	if err := pool.Reserve(cfg.GatewayIP, cfg.GatewayMacAddress); err != nil {
		return nil, nil, err
	}
	for address, mac := range cfg.DHCPStaticLeases {
		if err := pool.Reserve(address, mac); err != nil {
			return nil, nil, err
		}
	}
	link, err := tap.NewLinkEndpoint(false, mtu, gatewayMAC, cfg.GatewayIP, nil)
	if err != nil {
		return nil, nil, err
	}
	switcher := tap.NewSwitch(false)
	link.Connect(switcher)
	switcher.Connect(link)

	s := stack.New(stack.Options{
		NetworkProtocols:   []stack.NetworkProtocolFactory{ipv4.NewProtocol, arp.NewProtocol},
		TransportProtocols: []stack.TransportProtocolFactory{tcp.NewProtocol, udp.NewProtocol},
	})
	if err := s.CreateNIC(1, link); err != nil {
		return nil, nil, errors.New(err.String())
	}
	if err := s.AddProtocolAddress(1, tcpip.ProtocolAddress{
		Protocol: ipv4.ProtocolNumber, AddressWithPrefix: gateway.WithPrefix(),
	}, stack.AddressProperties{}); err != nil {
		s.Close()
		return nil, nil, errors.New(err.String())
	}
	subnet, err := tcpip.NewSubnet(tcpip.AddrFrom4(guestSubnet.Addr().As4()), tcpip.MaskFromBytes([]byte{255, 255, 255, 0}))
	if err != nil {
		s.Close()
		return nil, nil, err
	}
	s.SetSpoofing(1, true)
	s.SetPromiscuousMode(1, true)
	s.SetRouteTable([]tcpip.Route{{Destination: subnet, NIC: 1}})

	startForwarding(ctx, s)
	dhcpServer, err := dhcp.New(cfg, s, pool)
	if err != nil {
		s.Close()
		return nil, nil, fmt.Errorf("start guest DHCP: %w", err)
	}
	stopDNS, err := startDNS(ctx, s)
	if err != nil {
		dhcpServer.Underlying.Close()
		s.Close()
		return nil, nil, fmt.Errorf("start guest DNS: %w", err)
	}
	go func() { _ = dhcpServer.Serve() }()
	return func(ctx context.Context, conn net.Conn) error {
			return switcher.Accept(ctx, conn, types.VfkitProtocol)
		}, func() {
			stopDNS()
			dhcpServer.Underlying.Close()
			s.Close()
		}, nil
}

func dnsEndpoints(s *stack.Stack) (net.PacketConn, net.Listener, error) {
	address := tcpip.FullAddress{NIC: 1, Addr: gateway, Port: 53}
	datagrams, err := gonet.DialUDP(s, &address, nil, ipv4.ProtocolNumber)
	if err != nil {
		return nil, nil, err
	}
	streams, err := gonet.ListenTCP(s, address, ipv4.ProtocolNumber)
	if err != nil {
		datagrams.Close()
		return nil, nil, err
	}
	return datagrams, streams, nil
}
