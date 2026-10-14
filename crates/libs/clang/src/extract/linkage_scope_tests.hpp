#pragma once

#if defined(LINKAGE_ALTERNATIVE)
#define LINKAGE_VALUE unsigned int
#else
#define LINKAGE_VALUE int
#endif

#if defined(LINKAGE_COLLISION)
#define LINKAGE_A_NAME Named
#define LINKAGE_B_NAME Named
#else
#define LINKAGE_A_NAME NamedA
#define LINKAGE_B_NAME NamedB
#endif

typedef struct Plain { LINKAGE_VALUE value; } PlainAlias;
typedef struct PlainInterface PlainInterface;
struct __declspec(uuid("a3000000-0000-0000-0000-000000000001")) PlainInterface {
    virtual int __stdcall Value(LINKAGE_VALUE value) = 0;
};

extern "C" {
typedef struct Wrapped { LINKAGE_VALUE value; } WrappedAlias;
typedef struct WrappedInterface WrappedInterface;
struct __declspec(uuid("a3000000-0000-0000-0000-000000000002")) WrappedInterface {
    virtual int __stdcall Value(LINKAGE_VALUE value) = 0;
};
extern "C++" {
typedef struct Nested { LINKAGE_VALUE value; } NestedAlias;
}
struct Excluded { int unused; };
#if defined(LINKAGE_DEMAND)
struct Demand { Excluded value; };
#endif
}

namespace A {
extern "C" {
struct LINKAGE_A_NAME { int value; };
}
struct Outer { struct Inner; };
}
struct A::Outer::Inner { int value; };

namespace B {
extern "C++" {
struct LINKAGE_B_NAME { unsigned int value; };
}
}
