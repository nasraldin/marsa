package main

import (
	"errors"
	"net"
	"net/netip"
	"testing"
)

func TestOutboundAddressPolicy(t *testing.T) {
	hostInterfaces := func() ([]net.Addr, error) {
		ip, network, err := net.ParseCIDR("10.23.0.9/24")
		if err != nil {
			t.Fatal(err)
		}
		network.IP = ip
		return []net.Addr{network}, nil
	}
	for _, test := range []struct {
		address string
		port    uint16
		allow   bool
	}{
		{"0.1.2.3", 80, false},
		{"127.0.0.1", 443, false},
		{"127.200.19.11", 53, false},
		{"169.254.169.254", 80, false},
		{"169.254.170.2", 80, false},
		{"100.100.100.200", 80, false},
		{"168.63.129.16", 443, false},
		{"192.0.0.192", 80, false},
		{"192.168.127.1", 443, false},
		{"192.168.127.254", 443, false},
		{"10.23.0.9", 443, false},
		{"10.23.0.10", 443, true},
		{"203.0.113.123", 53, false},
		{"203.0.113.123", 443, true},
		{"224.0.0.1", 443, false},
		{"255.255.255.255", 443, false},
		{"fd00:ec2::254", 80, false},
		{"::ffff:127.0.0.1", 80, false},
	} {
		got := permitted(netip.MustParseAddr(test.address), test.port, hostInterfaces)
		if got != test.allow {
			t.Errorf("permitted(%s, %d) = %t, want %t", test.address, test.port, got, test.allow)
		}
	}
	if permitted(netip.MustParseAddr("203.0.113.123"), 443, func() ([]net.Addr, error) {
		return nil, errors.New("interface enumeration failed")
	}) {
		t.Fatal("failed interface enumeration must deny egress")
	}
}
