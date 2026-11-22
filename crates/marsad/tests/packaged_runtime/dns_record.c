// Publish one disposable DNS A record through the local-only macOS resolver.
#include <dns_sd.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>

static int registered = 0;

static void registered_record(DNSServiceRef service, DNSRecordRef record,
    DNSServiceFlags flags, DNSServiceErrorType error, void *context)
{
    (void)service;
    (void)record;
    (void)flags;
    (void)context;
    registered = error == kDNSServiceErr_NoError ? 1 : -1;
}

int main(int argc, char **argv)
{
    if (argc != 2) {
        return EXIT_FAILURE;
    }
    DNSServiceRef connection = NULL;
    DNSRecordRef record = NULL;
    const unsigned char first[] = {192, 0, 2, 64};
    const unsigned char second[] = {192, 0, 2, 65};
    DNSServiceErrorType result = DNSServiceCreateConnection(&connection);
    if (result != kDNSServiceErr_NoError) {
        fprintf(stderr, "DNSServiceCreateConnection: %d\n", result);
        return EXIT_FAILURE;
    }
    result = DNSServiceRegisterRecord(connection, &record, kDNSServiceFlagsUnique,
        kDNSServiceInterfaceIndexLocalOnly, argv[1], kDNSServiceType_A,
        kDNSServiceClass_IN, sizeof(first), first, 1, registered_record, NULL);
    struct pollfd event = { .fd = DNSServiceRefSockFD(connection), .events = POLLIN };
    while (result == kDNSServiceErr_NoError && registered == 0
        && poll(&event, 1, 3000) > 0 && (event.revents & POLLIN)) {
        result = DNSServiceProcessResult(connection);
    }
    if (result != kDNSServiceErr_NoError || registered != 1) {
        fprintf(stderr, "local DNS register: %d (%d)\n", result, registered);
        DNSServiceRefDeallocate(connection);
        return EXIT_FAILURE;
    }
    puts("registered");
    fflush(stdout);
    if (getchar() == '\n') {
        result = DNSServiceUpdateRecord(connection, record, 0,
            sizeof(second), second, 1);
        if (result == kDNSServiceErr_NoError) {
            puts("updated");
            fflush(stdout);
            (void)getchar();
        } else {
            fprintf(stderr, "local DNS update: %d\n", result);
        }
    }
    DNSServiceRefDeallocate(connection);
    return result == kDNSServiceErr_NoError ? EXIT_SUCCESS : EXIT_FAILURE;
}
