#pragma once

struct NativeId {
    unsigned int data1;
    unsigned short data2;
    unsigned short data3;
    unsigned char data4[8];
};

union NativeInteger {
    struct {
        unsigned int low;
        int high;
    };
    long long value;
};

struct __declspec(uuid("d10e84c3-5079-4d3d-8d9f-28f1ef9c643d")) NativeUnknown {
    virtual int __stdcall QueryInterface(const NativeId& id, void** value) = 0;
    virtual unsigned int __stdcall AddRef() = 0;
    virtual unsigned int __stdcall Release() = 0;
};
