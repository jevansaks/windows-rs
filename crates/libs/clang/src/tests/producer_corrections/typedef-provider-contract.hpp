#pragma once

#ifndef PROVIDER_CV
#define PROVIDER_CV
#endif
#ifndef PROVIDER_ALIAS_CV
#define PROVIDER_ALIAS_CV
#endif

struct ProviderPayload { long value; };
typedef PROVIDER_ALIAS_CV ProviderPayload PayloadAlias;
struct ProviderBase {
    virtual long __stdcall Observe(PROVIDER_CV PayloadAlias& value) = 0;
};
struct ProviderDerived : ProviderBase {
    virtual long __stdcall Finish() = 0;
};
