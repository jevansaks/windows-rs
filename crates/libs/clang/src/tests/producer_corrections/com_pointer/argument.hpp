#pragma once
#ifdef COM_ARGUMENT_INLINE_LAYOUT
#ifdef COM_ARGUMENT_MEMBER_FLOAT
#define COM_ARGUMENT_MEMBER_TYPE float
#elif defined(COM_ARGUMENT_MEMBER_DOUBLE)
#define COM_ARGUMENT_MEMBER_TYPE double
#else
#define COM_ARGUMENT_MEMBER_TYPE int
#endif
typedef COM_ARGUMENT_MEMBER_TYPE SplitPart;
#undef COM_ARGUMENT_MEMBER_TYPE
typedef union SplitValue {
    struct {
        unsigned int low;
        SplitPart high;
    };
    long long whole;
} SplitValue;
struct Argument {
    unsigned int flags;
    SplitValue value;
};
#else
struct Argument {
#ifdef COM_ARGUMENT_DOUBLE
    double value;
#else
    int value;
#endif
};
#endif
