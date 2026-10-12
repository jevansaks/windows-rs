#pragma once
#include "base.hpp"
typedef struct NativeContract NativeContract;
struct __declspec(uuid("6b44d453-343e-4246-826d-208c460655c5")) __declspec(novtable)
NativeContract : public NativeBase {
    virtual Status __stdcall Reset() = 0;
};
