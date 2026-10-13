#pragma once
struct __declspec(uuid("f5dd8cb0-3fca-4c46-a585-8b06a22d9799")) NativeProvider {
    virtual int __stdcall Keep(NativeProvider* value) = 0;
};
struct __declspec(uuid("5641eaf8-d4bb-410c-919a-0b7c82c6d7f2")) NativeConsumer : NativeProvider {
    virtual int __stdcall Connect(NativeProvider* value) = 0;
};
struct NonInterface {
    int value;
    virtual int __stdcall Keep() = 0;
};
typedef NonInterface* NonInterfacePointer;
typedef NativeProvider NativeProviderValue;
