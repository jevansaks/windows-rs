#pragma once

#ifndef TYPE_EXTRA
#define TYPE_EXTRA
#endif
#ifndef FIELD_EXTRA
#define FIELD_EXTRA
#endif
#ifndef NESTED_EXTRA
#define NESTED_EXTRA
#endif
#ifndef VARIANT_EXTRA
#define VARIANT_EXTRA
#endif
#ifndef TYPEDEF_EXTRA
#define TYPEDEF_EXTRA
#endif
#ifndef CALLBACK_EXTRA
#define CALLBACK_EXTRA
#endif
#ifndef RESULT_EXTRA
#define RESULT_EXTRA
#endif
#ifndef PARAMETER_EXTRA
#define PARAMETER_EXTRA
#endif

struct TYPE_EXTRA SourceRecord {
    int __attribute__((annotate("win32metadata:const"))) first;
    int FIELD_EXTRA second;
    struct {
        int NESTED_EXTRA nested;
    } inner;
};

enum class TYPE_EXTRA SourceEnum : unsigned int {
    First = 1,
    Second VARIANT_EXTRA = 2
};

typedef int SourceAlias TYPEDEF_EXTRA;
typedef int(__stdcall* SourceCallback)(
    __attribute__((annotate("win32metadata:in"))) CALLBACK_EXTRA int* value);

struct __declspec(uuid("79000000-0000-0000-0000-000000000010"))
    SourceInterface {
    virtual int __stdcall Overload(int value)
        __attribute__((annotate("win32metadata:preserve_result"))) = 0;
    virtual int __stdcall Overload(
        __attribute__((annotate("win32metadata:in"))) PARAMETER_EXTRA int* value)
        __attribute__((annotate("win32metadata:can_return_errors_as_success")))
        RESULT_EXTRA = 0;
};

#ifdef SOURCE_SCOPES
struct ScopeOne {
    struct __attribute__((annotate("win32metadata:supported_os=Windows10")))
        TYPE_EXTRA ScopedRecord {
        int FIELD_EXTRA value;
    };
};
struct ScopeTwo {
    struct __attribute__((annotate("win32metadata:supported_os=Windows11")))
        ScopedRecord {
        int value;
    };
};
#endif

constexpr int SourceConstant
    __attribute__((annotate("win32metadata:unicode"))) = 7;
extern "C" int SourceFunction(
    __attribute__((annotate("win32metadata:in"))) int* value)
    __attribute__((annotate("win32metadata:preserve_result")));
