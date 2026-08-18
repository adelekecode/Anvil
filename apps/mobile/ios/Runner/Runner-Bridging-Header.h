#import "GeneratedPluginRegistrant.h"
#include <stddef.h>
#include <stdint.h>

typedef uint32_t (*AnvilCapabilitiesCallback)(void *context);
typedef int32_t (*AnvilInvokeCallback)(
    void *context,
    const char *operation,
    uint64_t argument,
    const uint8_t *bytes,
    size_t length,
    const char *text);
typedef intptr_t (*AnvilLoadIdentityCallback)(
    void *context,
    uint8_t *buffer,
    size_t capacity);
typedef void (*AnvilReleaseCallback)(void *context);

typedef struct {
    void *context;
    AnvilCapabilitiesCallback capabilities;
    AnvilInvokeCallback invoke;
    AnvilLoadIdentityCallback load_identity;
    AnvilReleaseCallback release;
} AnvilPlatformCallbacks;

int32_t anvil_attach_platform(void *session, const AnvilPlatformCallbacks *callbacks);
void anvil_detach_platform(void *session);
int32_t anvil_submit_platform_event(void *session, const char *json);
