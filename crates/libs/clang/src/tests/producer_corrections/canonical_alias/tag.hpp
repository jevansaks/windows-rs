#pragma once
#define NATIVE_UUID(value) __declspec(uuid(value))
#define NATIVE_INTERFACE(name) struct __declspec(novtable) name
struct NATIVE_UUID("6b44d453-343e-4246-826d-208c460655c5") NativeTag;
typedef NativeTag NativeTag;
#ifdef NATIVE_TAG_COMPLETE
NATIVE_INTERFACE(NativeTag) : NativeBase {
#ifdef NATIVE_TAG_ALTERNATE
    virtual int __stdcall Call(double value) = 0;
#else
    virtual int __stdcall Call(int value) = 0;
#endif
#ifdef NATIVE_TAG_FIELDS
    int helper_state;
#endif
};
#endif
