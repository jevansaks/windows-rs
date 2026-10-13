#pragma once
#ifndef AUTHORITY_VALUE
#define AUTHORITY_VALUE long
#endif

struct AuthorityPayload { AUTHORITY_VALUE value; };
typedef AuthorityPayload AuthorityAlias;
struct __declspec(uuid("6a0ed4d0-1c04-437d-b6b3-8a1f53ce0581")) AuthorityBase {
    virtual long __stdcall QueryInterface(const AuthorityAlias& iid, void** object) = 0;
};

#if defined(AUTHORITY_ABSENT)
#elif defined(AUTHORITY_FORWARD)
struct __declspec(uuid("6a0ed4d0-1c04-437d-b6b3-8a1f53ce0582")) AuthorityInterface;
#else
struct __declspec(uuid("6a0ed4d0-1c04-437d-b6b3-8a1f53ce0582"))
AuthorityInterface : AuthorityBase {
#if defined(AUTHORITY_RECORD)
    int helper_state;
#endif
#if defined(AUTHORITY_UNREPRESENTABLE)
    virtual long __stdcall Value(int __attribute__((address_space(1)))* value) = 0;
#else
    virtual AUTHORITY_VALUE __stdcall Value(
        AUTHORITY_VALUE count, const AuthorityAlias& value) = 0;
#endif
#if defined(AUTHORITY_EXTRA)
    virtual long __stdcall AlternativeOnly() = 0;
#endif
};
#endif

#if defined(AUTHORITY_OFFSETS)
#if defined(AUTHORITY_ALTERNATIVE)
struct __declspec(uuid("6a0ed4d0-1c04-437d-b6b3-8a1f53ce0583")) AuthorityOffset {
    virtual unsigned long long __stdcall Alternative(unsigned long long value) = 0;
};
#else
struct __declspec(uuid("6a0ed4d0-1c04-437d-b6b3-8a1f53ce0583")) AuthorityOffset {
    virtual long __stdcall Home(long value) = 0;
};
#endif
#endif

#if defined(AUTHORITY_OPAQUE)
typedef struct AuthorityOpaque AuthorityOpaque;
struct AuthorityOpaque;
#if defined(AUTHORITY_ALTERNATIVE)
struct AuthorityOpaque { unsigned long long alternative_field; };
#endif
#endif

#if defined(AUTHORITY_SCOPE)
#if defined(AUTHORITY_ALTERNATIVE)
struct ScopeOuter { struct ScopeInner; };
struct ScopeOuter::ScopeInner { int value; };
#elif defined(AUTHORITY_SCOPE_OTHER)
struct ScopeOther { struct ScopeInner { int value; }; };
#else
struct ScopeOuter { struct ScopeInner { int value; }; };
#endif
#endif
