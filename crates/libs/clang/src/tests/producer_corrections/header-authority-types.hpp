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

#if defined(AUTHORITY_EVIDENCE)
typedef int EvidenceT;
typedef EvidenceT EvidenceA;
typedef const EvidenceT EvidenceCA;
typedef EvidenceT* const EvidenceCP;
#if defined(AUTHORITY_EVIDENCE_INLINE)
typedef int (__stdcall *EvidenceCallback)(int);
#endif

struct EvidenceNominal {
#if defined(AUTHORITY_ALTERNATIVE) && defined(AUTHORITY_EVIDENCE_NOMINAL)
    unsigned long long payload;
#else
    int payload;
#endif
};

#if defined(AUTHORITY_ALTERNATIVE) && defined(AUTHORITY_EVIDENCE_FORWARD)
struct EvidenceRecord;
#else
struct EvidenceRecord {
#if defined(AUTHORITY_EVIDENCE_SCALAR)
#if defined(AUTHORITY_ALTERNATIVE)
    unsigned long long value;
#else
    int value;
#endif
#elif defined(AUTHORITY_EVIDENCE_NOMINAL)
    EvidenceNominal* value;
#elif defined(AUTHORITY_EVIDENCE_INLINE)
    struct {
#if defined(AUTHORITY_ALTERNATIVE)
        unsigned int nested;
#else
        int nested;
#endif
    } value;
#if defined(AUTHORITY_EVIDENCE_DIRECT_CALLBACK)
#if defined(AUTHORITY_ALTERNATIVE) && defined(AUTHORITY_EVIDENCE_CALLBACK_DIFFERENT)
    int (__stdcall *callback)(unsigned int);
#else
    int (__stdcall *callback)(int);
#endif
#else
    EvidenceCallback callback;
#endif
#else
#if defined(AUTHORITY_ALTERNATIVE) && !defined(AUTHORITY_EVIDENCE_ALIAS_ALTERNATIVE)
    EvidenceT* plain;
    const EvidenceT* qualified;
#if defined(AUTHORITY_EVIDENCE_NEGATIVE)
    const EvidenceT** pointer_const;
#else
    EvidenceT* const* pointer_const;
#endif
#if defined(AUTHORITY_EVIDENCE_REFERENCES)
    const EvidenceT& qualified_ref;
#if defined(AUTHORITY_EVIDENCE_NEGATIVE)
    const EvidenceT*& pointer_const_ref;
#else
    EvidenceT* const& pointer_const_ref;
#endif
#endif
#else
    EvidenceA* plain;
    EvidenceCA* qualified;
    EvidenceCP* pointer_const;
#if defined(AUTHORITY_EVIDENCE_REFERENCES)
    EvidenceCA& qualified_ref;
    EvidenceCP& pointer_const_ref;
#endif
#endif
#endif
};
#endif
#endif
