//! library kernel32.dll

#define W32M(text) [[clang::annotate(text)]]
#define W32M_VALUE(key, value) [[clang::annotate("win32metadata:" key "=" value)]]

typedef void* HANDLE;
typedef int BOOL;
typedef unsigned long DWORD;

W32M("win32metadata:also_usable_for=HANDLE")
typedef HANDLE DISTINCT_HANDLE;

W32M("win32metadata:set_last_error")
W32M("win32metadata:supported_os=windows10.0.10240")
W32M("win32metadata:can_return_multiple_success_values")
BOOL AnnotatedFunction(
    W32M("win32metadata:raii_free=CloseHandle")
    W32M("win32metadata:invalid_handle=-1")
    W32M("win32metadata:invalid_handle=0")
    HANDLE* result,
    W32M("win32metadata:not_null_terminated")
    const char* bytes,
    DWORD count);

W32M("win32metadata:raii_free=CloseHandle")
W32M("win32metadata:invalid_handle=-1")
HANDLE AnnotatedReturn(void);

typedef struct ANNOTATED_STRUCT {
    W32M("win32metadata:associated_enum=FLAGS")
    DWORD flags;
} ANNOTATED_STRUCT;
