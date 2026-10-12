#pragma once
namespace Generic {
struct Record { MEMBER_TYPE value; unsigned int bits : 3; const volatile long fixed[2]; };
union Parts { unsigned long long whole; struct { int low; MEMBER_TYPE high; }; };
enum Enumeration : MEMBER_TYPE { First = -1, Second = ENUM_VALUE };
enum class Scoped : long { First = 1, Second = 4 };
typedef Record RecordAlias;
typedef Parts UnionAlias;
typedef Enumeration EnumAlias;
typedef Scoped ScopedAlias;
struct Incomplete;
typedef Incomplete IncompleteAlias;
struct Recursive { Recursive* next; };
typedef Recursive RecursiveAlias;
struct NonPod { virtual ~NonPod(); int value; };
typedef NonPod NonPodAlias;
struct __declspec(align(16)) Attributed { int value; };
typedef Attributed AttributedAlias;
struct __declspec(uuid("c9f27973-f120-408b-bc6d-3a6f5bdf4e61")) Interface {
    virtual RecordAlias __stdcall Transform(UnionAlias value, EnumAlias kind) = 0;
};
}
