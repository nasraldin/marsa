//go:build darwin && cgo

package main

/*
#include <stdlib.h>
#include "resolver_darwin.h"
*/
import "C"

import (
	"encoding/binary"
	"unsafe"

	"github.com/miekg/dns"
)

func systemResolve(question dns.Question) ([]dns.RR, int) {
	name := C.CString(question.Name)
	defer C.free(unsafe.Pointer(name))

	var result C.struct_marsa_query
	switch C.marsa_scoped_dns(name, C.uint16_t(question.Qtype), &result) {
	case 0:
	case 1:
		return nil, dns.RcodeSuccess
	default:
		return nil, dns.RcodeServerFailure
	}

	answers := make([]dns.RR, 0, int(result.count))
	for _, record := range result.records[:result.count] {
		wire := make([]byte, 255+10+int(record.length))
		owner := C.GoString(&record.name[0])
		offset, err := dns.PackDomainName(owner, wire, 0, nil, false)
		if err != nil {
			return nil, dns.RcodeServerFailure
		}
		header := wire[offset : offset+10]
		binary.BigEndian.PutUint16(header, uint16(record._type))
		binary.BigEndian.PutUint16(header[2:], uint16(record.rrclass))
		binary.BigEndian.PutUint32(header[4:], uint32(record.ttl))
		binary.BigEndian.PutUint16(header[8:], uint16(record.length))
		copy(wire[offset+10:], C.GoBytes(unsafe.Pointer(&record.data[0]), C.int(record.length)))
		rr, end, err := dns.UnpackRR(wire[:offset+10+int(record.length)], 0)
		if err != nil || end != offset+10+int(record.length) {
			return nil, dns.RcodeServerFailure
		}
		answers = append(answers, rr)
	}
	return answers, dns.RcodeSuccess
}
