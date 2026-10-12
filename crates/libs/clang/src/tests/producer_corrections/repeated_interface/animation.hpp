#pragma once
#include "base.hpp"
#define interface struct
#define NATIVE_MIDL_INTERFACE(value) struct __declspec(uuid(value)) __declspec(novtable)

typedef interface NativeAnimation NativeAnimation;

extern "C" {
NATIVE_MIDL_INTERFACE("6b44d453-343e-4246-826d-208c460655c5")
NativeAnimation : public NativeUnknown {
    virtual int __stdcall Reset() = 0;
    virtual int __stdcall SetAbsoluteBeginTime(NativeInteger begin_time) = 0;
    virtual int __stdcall AddCubic(double offset, float constant, float linear,
                                  float quadratic, float cubic) = 0;
    virtual int __stdcall AddSinusoidal(double offset, float bias, float amplitude,
                                       float frequency, float phase) = 0;
    virtual int __stdcall AddRepeat(double offset, double duration) = 0;
    virtual int __stdcall End(double offset, float value) = 0;
};
}
