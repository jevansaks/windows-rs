#pragma once
struct __declspec(uuid("d10e84c3-5079-4d3d-8d9f-28f1ef9c643d")) NativeBase {
    virtual int __stdcall QueryInterface(const void* id, void** value) = 0;
    virtual unsigned int __stdcall AddRef() = 0;
    virtual unsigned int __stdcall Release() = 0;
};
