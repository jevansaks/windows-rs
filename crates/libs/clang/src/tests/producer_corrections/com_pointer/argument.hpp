#pragma once
struct Argument {
#ifdef COM_ARGUMENT_DOUBLE
    double value;
#else
    int value;
#endif
};
