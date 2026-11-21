package main

import (
	"net"
	"testing"

	"github.com/miekg/dns"
)

type responseRecorder struct{ response *dns.Msg }

func (*responseRecorder) LocalAddr() net.Addr       { return nil }
func (*responseRecorder) RemoteAddr() net.Addr      { return nil }
func (*responseRecorder) Write([]byte) (int, error) { return 0, nil }
func (*responseRecorder) Close() error              { return nil }
func (*responseRecorder) TsigStatus() error         { return nil }
func (*responseRecorder) TsigTimersOnly(bool)       {}
func (*responseRecorder) Hijack()                   {}
func (w *responseRecorder) WriteMsg(reply *dns.Msg) error {
	w.response = reply
	return nil
}

func TestDNSUsesOnlyScopedSource(t *testing.T) {
	var queries []dns.Question
	resolve := func(q dns.Question) ([]dns.RR, int) {
		queries = append(queries, q)
		return nil, dns.RcodeServerFailure
	}
	handle := dnsHandler(resolve, true, make(chan struct{}, 1))
	w := &responseRecorder{}
	request := new(dns.Msg).SetQuestion("internal.example.", dns.TypeHTTPS)
	handle(w, request)
	if w.response.Rcode != dns.RcodeServerFailure || len(queries) != 1 || queries[0] != request.Question[0] {
		t.Fatalf("scoped failure was not preserved: %v, %v", w.response, queries)
	}
	handle(w, new(dns.Msg).SetQuestion("internal.example.", dns.TypeAXFR))
	if w.response.Rcode != dns.RcodeNotImplemented || len(queries) != 1 {
		t.Fatal("zone transfer should fail before resolution")
	}
}
