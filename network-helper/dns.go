package main

import (
	"context"
	"errors"
	"strings"
	"time"

	"github.com/miekg/dns"
	"gvisor.dev/gvisor/pkg/tcpip/stack"
)

type resolveDNS func(dns.Question) ([]dns.RR, int)

const maxDNSQueries = 16

func dnsHandler(resolve resolveDNS, udp bool, slots chan struct{}) dns.HandlerFunc {
	return func(w dns.ResponseWriter, request *dns.Msg) {
		reply := new(dns.Msg).SetReply(request)
		reply.RecursionAvailable = true
		switch {
		case request.Response || request.Opcode != dns.OpcodeQuery || len(request.Question) != 1:
			reply.Rcode = dns.RcodeFormatError
		case request.Question[0].Qclass != dns.ClassINET || !supportedQuestion(request.Question[0]):
			reply.Rcode = dns.RcodeNotImplemented
		default:
			select {
			case slots <- struct{}{}:
				reply.Answer, reply.Rcode = resolve(request.Question[0])
				<-slots
			default:
				reply.Rcode = dns.RcodeServerFailure
			}
		}
		if udp {
			size := dns.MinMsgSize
			if opt := request.IsEdns0(); opt != nil {
				size = min(1232, max(int(opt.UDPSize()), size))
			}
			reply.Truncate(size)
		}
		_ = w.WriteMsg(reply)
	}
}

func supportedQuestion(q dns.Question) bool {
	// These QTYPEs carry protocol operations, not an ordinary lookup.
	if q.Qtype == 0 || q.Qtype == dns.TypeANY || q.Qtype == dns.TypeAXFR ||
		q.Qtype == dns.TypeIXFR || q.Qtype == dns.TypeOPT || q.Qtype == dns.TypeTSIG ||
		q.Qtype == dns.TypeTKEY || q.Qtype == dns.TypeMAILA || q.Qtype == dns.TypeMAILB {
		return false
	}
	if len(q.Name) > 254 || !dns.IsFqdn(q.Name) {
		return false
	}
	// Treat unescaped UTF-8, separators and control bytes as unsupported.
	// IDNA names arrive as ASCII A-labels. Underscores support SRV and TXT.
	return strings.Trim(q.Name, "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_.") == ""
}

func startDNS(ctx context.Context, s *stack.Stack) (func(), error) {
	datagrams, streams, err := dnsEndpoints(s)
	if err != nil {
		return nil, err
	}
	started := make(chan struct{}, 2)
	failed := make(chan error, 2)
	notifyStarted := func() { started <- struct{}{} }
	slots := make(chan struct{}, maxDNSQueries)
	udpServer := &dns.Server{
		PacketConn: datagrams, Handler: dnsHandler(systemResolve, true, slots),
		NotifyStartedFunc: notifyStarted, UDPSize: 1232, WriteTimeout: 4 * time.Second,
	}
	tcpServer := &dns.Server{
		Listener: streams, Handler: dnsHandler(systemResolve, false, slots),
		NotifyStartedFunc: notifyStarted, MaxTCPQueries: 16,
		ReadTimeout: 4 * time.Second, WriteTimeout: 4 * time.Second,
	}
	stop := func() {
		// Closing the virtual endpoints prevents shutdown from waiting on a
		// guest that keeps a DNS connection open.
		_ = datagrams.Close()
		_ = streams.Close()
	}
	for _, server := range []*dns.Server{udpServer, tcpServer} {
		go func() { failed <- server.ActivateAndServe() }()
	}
	for range 2 {
		select {
		case <-started:
		case err := <-failed:
			stop()
			if err == nil {
				err = errors.New("guest DNS service stopped before readiness")
			}
			return nil, err
		case <-ctx.Done():
			stop()
			return nil, ctx.Err()
		}
	}
	return stop, nil
}
