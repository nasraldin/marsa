//go:build darwin && cgo

#include "resolver_darwin.h"
#include <dns_sd.h>
#include <errno.h>
#include <poll.h>
#include <string.h>
#include <time.h>

static void receive_record(
    DNSServiceRef service, DNSServiceFlags flags, uint32_t interface_index,
    DNSServiceErrorType error, const char *name, uint16_t type, uint16_t rrclass,
    uint16_t length, const void *data, uint32_t ttl, void *context)
{
    (void)service;
    (void)interface_index;
    struct marsa_query *query = context;
    if (query->complete) {
        return;
    }
    if (error != kDNSServiceErr_NoError) {
        query->status = error == kDNSServiceErr_NoSuchRecord ? 1 : -1;
        query->complete = 1;
        return;
    }
    if (!(flags & kDNSServiceFlagsAdd)) {
        return;
    }
    if ((type != query->requested_type && type != kDNSServiceType_CNAME) ||
        !name || !data || strnlen(name, 256) >= 256 || length > marsa_rdata_max ||
        query->count >= marsa_record_max) {
        query->status = -1;
        query->complete = 1;
        return;
    }
    struct marsa_record *record = &query->records[query->count++];
    memcpy(record->name, name, strlen(name) + 1);
    record->type = type;
    record->rrclass = rrclass;
    record->length = length;
    record->ttl = ttl;
    memcpy(record->data, data, length);

    if (type == query->requested_type) {
        query->found_requested_type = 1;
    }
    if (!(flags & kDNSServiceFlagsMoreComing) && query->found_requested_type) {
        query->complete = 1;
    }
}

static int64_t monotonic_millis(void)
{
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) {
        return -1;
    }
    return (int64_t)now.tv_sec * 1000 + now.tv_nsec / 1000000;
}

int marsa_scoped_dns(const char *name, uint16_t type, struct marsa_query *query)
{
    DNSServiceRef service = NULL;
    query->requested_type = type;
    int64_t start = monotonic_millis();
    if (start < 0) {
        return -1;
    }
    // InterfaceIndexAny respects the system's unicast scoped and supplemental
    // resolvers. No interface, nameserver or raw fallback is supplied.
    DNSServiceErrorType error = DNSServiceQueryRecord(
        &service, kDNSServiceFlagsTimeout | kDNSServiceFlagsReturnIntermediates,
        kDNSServiceInterfaceIndexAny, name, type, kDNSServiceClass_IN, receive_record, query);
    if (error != kDNSServiceErr_NoError) {
        return -1;
    }

    struct pollfd notification = { .fd = DNSServiceRefSockFD(service), .events = POLLIN };
    while (!query->complete && notification.fd >= 0) {
        int64_t now = monotonic_millis();
        if (now < start || now - start >= 3000) {
            break;
        }
        int result = poll(&notification, 1, (int)(3000 - (now - start)));
        if (result < 0 && errno == EINTR) {
            continue;
        }
        if (result <= 0 || !(notification.revents & POLLIN) ||
            DNSServiceProcessResult(service) != kDNSServiceErr_NoError) {
            break;
        }
    }
    DNSServiceRefDeallocate(service);
    return query->complete ? query->status : -1;
}
