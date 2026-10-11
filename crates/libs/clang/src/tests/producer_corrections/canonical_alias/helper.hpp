#pragma once
#define NATIVE_UUID(value) __declspec(uuid(value))
struct NATIVE_UUID("d10e84c3-5079-4d3d-8d9f-28f1ef9c643d") NativeBase {
    virtual int __stdcall BaseCall() = 0;
};
struct NATIVE_UUID("6b44d453-343e-4246-826d-208c460655c5") NativeTag;
typedef struct NativeTag NativeTag;
