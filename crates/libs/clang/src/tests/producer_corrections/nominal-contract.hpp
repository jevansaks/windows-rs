#pragma once
typedef FIELD_KIND Leaf;
struct Plain { Leaf value; };
union SharedParts { unsigned long long whole; struct { int low; Leaf high; }; };
enum PlainEnum : ENUM_KIND { First = 1, Second = 2 };
namespace Payload { struct ScopedRecord { long value; }; }
typedef Plain PlainAlias;
typedef SharedParts PartsAlias;
typedef PlainEnum EnumAlias;
typedef Payload::ScopedRecord ScopedAlias;
struct __declspec(uuid("a7a60184-8028-413a-bde9-4b724d5945cd")) NominalContract {
    virtual PlainAlias __stdcall Transform(PartsAlias parts, EnumAlias kind, ScopedAlias scoped) = 0;
};
