#pragma once

#if defined(BODY_ALTERNATIVE)
typedef int BodyInt;
typedef const BodyInt BodyConst;
#else
typedef int BodyInt;
typedef const BodyInt BodyConst;
#endif

struct __declspec(uuid("75000000-0000-0000-0000-000000000001")) BodyBase {
    virtual long __stdcall Base(int value) = 0;
};

#if defined(BODY_OTHER_SCOPE)
namespace Other {
#endif

#if defined(BODY_FORWARD)
struct BodyInterface;
struct BodyInterface;
enum class BodyEnum : unsigned int;
enum class BodyEnum : unsigned int;
#else

#if defined(BODY_REDECLARATIONS)
struct BodyInterface;
struct BodyInterface;
enum class BodyEnum : unsigned int;
enum class BodyEnum : unsigned int;
#endif

#if defined(BODY_ANNOTATIONS)
#define BODY_ATTRIBUTES __attribute__((annotate("win32metadata:preserve_result")))
#elif defined(BODY_EQUIVALENT)
#define BODY_ATTRIBUTES \
    __attribute__((annotate("win32metadata:can_return_errors_as_success"))) \
    __attribute__((annotate("win32metadata:preserve_result")))
#else
#define BODY_ATTRIBUTES \
    __attribute__((annotate("win32metadata:preserve_result"))) \
    __attribute__((annotate("win32metadata:can_return_errors_as_success")))
#endif

#if defined(BODY_IID)
struct BodyInterface : BodyBase {
#else
struct __declspec(uuid("75000000-0000-0000-0000-000000000002")) BodyInterface : BodyBase {
#endif
#if defined(BODY_RECORD)
    int state;
#endif
#if defined(BODY_SIGNATURE)
    virtual long long __stdcall Value(
        __attribute__((annotate("_In_"))) unsigned int* other, int count) BODY_ATTRIBUTES = 0;
#elif defined(BODY_EQUIVALENT)
    virtual long __stdcall Value(
        __attribute__((annotate("_In_"))) const int* value, int count) BODY_ATTRIBUTES = 0;
#else
    virtual long __stdcall Value(
        __attribute__((annotate("_In_"))) BodyConst* value, int count) BODY_ATTRIBUTES = 0;
#endif
#if defined(BODY_METHOD_ORDER)
    virtual long __stdcall Overload(unsigned int value) = 0;
    virtual long __stdcall Overload(int value) = 0;
#else
    virtual long __stdcall Overload(int value) = 0;
    virtual long __stdcall Overload(unsigned int value) = 0;
#endif
#if defined(BODY_SPECIAL)
    virtual /* [propget] */
#else
    virtual
#endif
    long __stdcall Property(int value) = 0;
};

#if defined(BODY_SIGNED_ENUM)
#if defined(BODY_ENUM_VALUE)
enum class BodyEnum : int { First = 2, Last = -1 };
#else
enum class BodyEnum : int { First = 1, Last = -1 };
#endif
#elif defined(BODY_ENUM_SHAPE)
enum BodyEnum { First = 1, Last = -1 };
#elif defined(BODY_ENUM_REPR)
enum class BodyEnum : unsigned long long { First = 1, Last = 0xffffffffULL };
#elif defined(BODY_ENUM_ORDER)
enum class BodyEnum : unsigned int { Last = 0xffffffffU, First = 1 };
#elif defined(BODY_ENUM_VALUE)
enum class BodyEnum : unsigned int { First = 2, Last = 0xffffffffU };
#else
enum class BodyEnum : unsigned int { First = 1, Last = 0xffffffffU };
#endif
#endif

#if defined(BODY_IID)
#define DEFINE_GUID(name, a, b, c, d, e, f, g, h, i, j, k) static const int name = 0
DEFINE_GUID(IID_BodyInterface, 0x75000000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2);
#endif

#if defined(BODY_SHARED)
typedef enum { SharedOne = 1, SharedTwo = 2 } BodyShared;
#endif

#if defined(BODY_EMPTY)
#if defined(BODY_ALTERNATIVE)
enum class BodyEmpty : unsigned int;
#else
enum class BodyEmpty : unsigned int {};
#endif
#endif

#if defined(BODY_OTHER_SCOPE)
}
#endif
