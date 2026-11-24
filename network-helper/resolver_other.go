//go:build !darwin || !cgo

package main

import "github.com/miekg/dns"

// Non-macOS test builds never bypass the scoped resolver.
func systemResolve(dns.Question) ([]dns.RR, int) { return nil, dns.RcodeServerFailure }
