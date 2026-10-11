#pragma once
#include "base.hpp"
#include "argument.hpp"
#ifdef COM_WRONG_GUID
struct __declspec(uuid("6b44d453-343e-4246-826d-208c460655c6")) NativeTag;
#else
struct __declspec(uuid("6b44d453-343e-4246-826d-208c460655c5")) NativeTag;
#endif
typedef struct NativeTag NativeTag;
#ifdef COM_CONCRETE
#define COM_PURE
#else
#define COM_PURE = 0
#endif
#ifdef COM_WRONG_BASE
struct __declspec(novtable) NativeTag : NativeOtherBase {
#elif defined(COM_MULTIPLE_BASE)
struct __declspec(novtable) NativeTag : NativeBase, NativeOtherBase {
#elif defined(COM_VIRTUAL_BASE)
struct __declspec(novtable) NativeTag : virtual NativeBase {
#else
struct __declspec(novtable) NativeTag : NativeBase {
#endif
    virtual int COM_CALL BaseCall(const NativeId& helper_id, void** helper_result) COM_PURE;
#ifdef COM_VIRTUAL_DESTRUCTOR
    virtual ~NativeTag() = default;
#endif
#ifdef COM_VIRTUAL_CONVERSION
    virtual operator int() = 0;
#endif
#ifdef COM_REORDER
    virtual int COM_CALL GetPeer(NativeTag** helper_result) COM_PURE;
#endif
#ifdef COM_WRONG_RESULT
    virtual double COM_CALL Call(Argument* helper_value) COM_PURE;
#else
    virtual int COM_CALL Call(Argument* helper_value) COM_PURE;
#endif
#ifndef COM_REORDER
    virtual int COM_CALL GetPeer(NativeTag** helper_result) COM_PURE;
#endif
    int helper_state[2];
};
