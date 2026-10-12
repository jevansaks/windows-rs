#pragma once
#include "base.hpp"
#include "argument.hpp"

#ifdef COM_GRAPH_LATE_TYPE
#define COM_GRAPH_VALUE_TYPE float
#else
#define COM_GRAPH_VALUE_TYPE int
#endif
typedef COM_GRAPH_VALUE_TYPE GraphValue;
#undef COM_GRAPH_VALUE_TYPE

#ifdef COM_GRAPH_LATE_SLOT
#define COM_GRAPH_CHECK CheckOther
#else
#define COM_GRAPH_CHECK Check
#endif
#ifdef COM_GRAPH_LATE_CDECL
#define COM_GRAPH_CHECK_CALL __cdecl
#else
#define COM_GRAPH_CHECK_CALL COM_CALL
#endif
#ifdef COM_GRAPH_HELPER
#define COM_GRAPH_STATE int helper_state[2];
#else
#define COM_GRAPH_STATE
#endif

struct __declspec(uuid("6b44d453-343e-4246-826d-208c460655c7")) NativeParent;
struct __declspec(uuid("6b44d453-343e-4246-826d-208c460655c5")) NativeDevice;
struct __declspec(uuid("6b44d453-343e-4246-826d-208c460655c8")) NativeSurface;
typedef struct NativeParent NativeParent;
typedef struct NativeDevice NativeDevice;
typedef struct NativeSurface NativeSurface;

struct __declspec(novtable) NativeParent : NativeBase {
    virtual int COM_CALL CreateDevice(COM_SAL("_COM_Outptr_") NativeDevice** result) = 0;
    virtual int COM_GRAPH_CHECK_CALL COM_GRAPH_CHECK(GraphValue value, Argument* detail) = 0;
    COM_GRAPH_STATE
};
struct __declspec(novtable) NativeDevice : NativeBase {
    virtual int COM_CALL GetParent(COM_SAL("_COM_Outptr_") NativeParent** result) = 0;
    virtual int COM_CALL SetSurface(NativeSurface* value) = 0;
    virtual int COM_GRAPH_CHECK_CALL COM_GRAPH_CHECK(GraphValue value, Argument* detail) = 0;
    COM_GRAPH_STATE
};
struct __declspec(novtable) NativeSurface : NativeBase {
    virtual int COM_CALL GetDevice(COM_SAL("_COM_Outptr_") NativeDevice** result) = 0;
    virtual int COM_GRAPH_CHECK_CALL COM_GRAPH_CHECK(GraphValue value, Argument* detail) = 0;
    COM_GRAPH_STATE
};
