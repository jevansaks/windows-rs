#pragma once
#include "base.hpp"
#include "argument.hpp"
struct __declspec(uuid("6b44d453-343e-4246-826d-208c460655c5")) NativeTag;
typedef struct NativeTag NativeTag;
struct __declspec(novtable) NativeTag : NativeBase {
    virtual int COM_CALL BaseCall(const NativeId& id,
                                 COM_SAL("_COM_Outptr_") void** result) = 0;
    virtual int COM_CALL Call(Argument* value) = 0;
    virtual int COM_CALL GetPeer(COM_SAL("_COM_Outptr_") NativeTag** value) = 0;
};
