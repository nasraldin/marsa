#ifndef MARSA_NET_HELPER_RESOLVER_H
#define MARSA_NET_HELPER_RESOLVER_H
#include <stdint.h>

enum { marsa_record_max = 32, marsa_rdata_max = 4096 };

struct marsa_record {
    char name[256];
    uint16_t type;
    uint16_t rrclass;
    uint16_t length;
    uint32_t ttl;
    uint8_t data[marsa_rdata_max];
};

struct marsa_query {
    int count;
    int status;
    int complete;
    uint16_t requested_type;
    int found_requested_type;
    struct marsa_record records[marsa_record_max];
};

// Returns 0 with answers, 1 on NOERROR/NODATA, and -1 on failure.
int marsa_scoped_dns(const char *name, uint16_t type, struct marsa_query *query);
#endif
