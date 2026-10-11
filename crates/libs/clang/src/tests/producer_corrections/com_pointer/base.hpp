#pragma once
#ifdef COM_CDECL
#define COM_CALL __cdecl
#else
#define COM_CALL __stdcall
#endif
#define COM_SAL(text) __attribute__((annotate(text)))
struct NativeId {
    unsigned int first;
    unsigned short second;
    unsigned short third;
    unsigned char fourth[8];
};
struct __declspec(uuid("d10e84c3-5079-4d3d-8d9f-28f1ef9c643d")) NativeBase {
    virtual int COM_CALL BaseCall(const NativeId& id,
                                 COM_SAL("_COM_Outptr_") void** result) = 0;
};
#if defined(COM_WRONG_BASE) || defined(COM_MULTIPLE_BASE)
struct __declspec(uuid("d10e84c3-5079-4d3d-8d9f-28f1ef9c643e")) NativeOtherBase {
    virtual int COM_CALL BaseCall(const NativeId& id, void** result) = 0;
};
#endif
