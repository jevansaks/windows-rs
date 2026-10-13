#pragma once

struct __declspec(uuid("79000000-0000-0000-0000-000000000001")) AnnotationBase {
    virtual long __stdcall Base(int value) = 0;
};

#if defined(ANNOTATION_ALTERNATIVE) && defined(ANNOTATION_METHOD_CHANGE)
#define METHOD_ATTRIBUTES __attribute__((annotate("win32metadata:preserve_result")))
#elif defined(ANNOTATION_ALTERNATIVE) && defined(ANNOTATION_EQUIVALENT)
#define METHOD_ATTRIBUTES \
    __attribute__((annotate("win32metadata:can_return_errors_as_success"))) \
    __attribute__((annotate("win32metadata:preserve_result"))) \
    __attribute__((annotate("win32metadata:preserve_result")))
#else
#define METHOD_ATTRIBUTES \
    __attribute__((annotate("win32metadata:preserve_result"))) \
    __attribute__((annotate("win32metadata:can_return_errors_as_success")))
#endif

#if defined(ANNOTATION_ALTERNATIVE) && defined(ANNOTATION_METHOD_UNKNOWN)
#define METHOD_EXTRA __attribute__((annotate("win32metadata:static_library=review.lib")))
#else
#define METHOD_EXTRA
#endif

#if defined(ANNOTATION_ALTERNATIVE) && defined(ANNOTATION_PARAMETER_EXPRESSION)
#define PARAMETER_ATTRIBUTES __attribute__((annotate("_Out_writes_(count + 1)")))
#elif defined(ANNOTATION_ALTERNATIVE) && defined(ANNOTATION_PARAMETER_CHANGE)
#define PARAMETER_ATTRIBUTES __attribute__((annotate("_Out_")))
#elif defined(ANNOTATION_EQUAL_COUNT)
#if defined(ANNOTATION_ALTERNATIVE)
#define PARAMETER_ATTRIBUTES __attribute__((annotate("_In_reads_(count + 1)")))
#else
#define PARAMETER_ATTRIBUTES __attribute__((annotate("_In_reads_(count)")))
#endif
#else
#define PARAMETER_ATTRIBUTES __attribute__((annotate("_In_")))
#endif

#if defined(ANNOTATION_ALTERNATIVE) && defined(ANNOTATION_PARAMETER_UNKNOWN)
#define PARAMETER_EXTRA __attribute__((annotate("win32metadata:ignore_if_return=unsupported")))
#elif defined(ANNOTATION_COUNTS)
#if defined(ANNOTATION_ALTERNATIVE) && defined(ANNOTATION_COUNT_INVALID)
#define PARAMETER_EXTRA \
    __attribute__((annotate("win32metadata:array_count_const=2"))) \
    __attribute__((annotate("win32metadata:memory_size_const=8"))) \
    __attribute__((annotate("win32metadata:optional")))
#elif defined(ANNOTATION_ALTERNATIVE)
#define PARAMETER_EXTRA \
    __attribute__((annotate("win32metadata:memory_size_const=8"))) \
    __attribute__((annotate("win32metadata:array_count_const=2")))
#else
#define PARAMETER_EXTRA \
    __attribute__((annotate("win32metadata:array_count_const=2"))) \
    __attribute__((annotate("win32metadata:memory_size_const=8")))
#endif
#else
#define PARAMETER_EXTRA
#endif

struct __declspec(uuid("79000000-0000-0000-0000-000000000002"))
    AnnotationInterface : AnnotationBase {
    virtual long __stdcall Value(
        PARAMETER_ATTRIBUTES PARAMETER_EXTRA int* value, int count)
        METHOD_ATTRIBUTES METHOD_EXTRA = 0;
};

#if !defined(ANNOTATION_ENUM_DOMAIN)
#define ENUM_ATTRIBUTES
#elif defined(ANNOTATION_ALTERNATIVE) && defined(ANNOTATION_ENUM_CHANGE)
#define ENUM_ATTRIBUTES \
    __attribute__((annotate("win32metadata:supported_os=Windows11")))
#else
#define ENUM_ATTRIBUTES \
    __attribute__((annotate("win32metadata:supported_os=Windows10")))
#endif

#if defined(ANNOTATION_ALTERNATIVE) && defined(ANNOTATION_ENUM_UNKNOWN)
#define ENUM_EXTRA __attribute__((annotate("win32metadata:static_library=review.lib")))
#define VARIANT_EXTRA __attribute__((annotate("win32metadata:ignore_if_return=unsupported")))
#else
#define ENUM_EXTRA
#define VARIANT_EXTRA
#endif

enum class ENUM_ATTRIBUTES ENUM_EXTRA AnnotationEnum : unsigned int {
    First VARIANT_EXTRA = 5,
    Maximum = 0xffffffffu
};
