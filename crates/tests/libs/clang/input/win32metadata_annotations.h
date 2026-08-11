//! library kernel32.dll

#define W32M(text) [[clang::annotate(text)]]
#define W32M_POSTFIX(text) __attribute__((annotate(text)))
#define W32M_VALUE(key, value) [[clang::annotate("win32metadata:" key "=" value)]]

typedef void* HANDLE;
typedef int BOOL;
typedef unsigned long DWORD;
typedef unsigned char BYTE;
#define WINAPI __stdcall

enum class
    W32M_POSTFIX("win32metadata:supported_os=windows10.0.10240")
    W32M_POSTFIX("win32metadata:associated_constant=FLAGS_ALL")
    [[clang::flag_enum]]
    FLAGS : unsigned long {
    FLAGS_NONE = 0,
    FLAGS_ONE = 1,
};

W32M("win32metadata:also_usable_for=HANDLE")
typedef HANDLE DISTINCT_HANDLE;

W32M("win32metadata:raii_free=CloseHandle")
W32M("win32metadata:invalid_handle=-1")
W32M("win32metadata:invalid_handle=0")
typedef HANDLE RESOURCE_HANDLE;

W32M("win32metadata:canonical_name=PUBLIC_CALLBACK")
typedef BOOL (WINAPI *INTERNAL_CALLBACK)(DWORD value);

W32M("win32metadata:native_encoding=custom")
const char* const AMBIGUOUS_TEXT = "text";

W32M("win32metadata:associated_enum=FLAGS")
const DWORD AMBIGUOUS_FLAG = 1;

struct
    W32M_POSTFIX("win32metadata:agile")
    __declspec(uuid("40000000-0000-0000-c000-000000000046"))
    IAnnotated {
    virtual BOOL
        W32M_POSTFIX("win32metadata:unicode")
        Method(void) = 0;
};

W32M("win32metadata:set_last_error")
W32M("win32metadata:supported_os=windows10.0.10240")
W32M("win32metadata:can_return_errors_as_success")
W32M("win32metadata:can_return_multiple_success_values")
W32M("win32metadata:import_library=override.dll")
W32M("win32metadata:static_library=example.lib")
W32M("win32metadata:ansi")
BOOL AnnotatedFunction(
    W32M("win32metadata:raii_free=CloseHandle")
    W32M("win32metadata:invalid_handle=-1")
    W32M("win32metadata:invalid_handle=0")
    HANDLE* result,
    W32M("win32metadata:not_null_terminated")
    const char* bytes,
    DWORD count);

BOOL AnnotatedParameters(
    HANDLE* allocated W32M_POSTFIX("win32metadata:free_with=CloseHandle"),
    HANDLE* ignored W32M_POSTFIX("win32metadata:ignore_if_return=0"),
    char* text
        W32M_POSTFIX("win32metadata:not_null_terminated")
        W32M_POSTFIX("win32metadata:null_null_terminated"),
    void* items W32M_POSTFIX("win32metadata:array_count_param=5"),
    void* fixed W32M_POSTFIX("win32metadata:array_count_const=0x10"),
    DWORD count,
    void* bytes W32M_POSTFIX("win32metadata:memory_size_param=7"),
    DWORD byteCount,
    HANDLE* retained W32M_POSTFIX("win32metadata:retained"),
    HANDLE* direction
        W32M_POSTFIX("win32metadata:in")
        W32M_POSTFIX("win32metadata:out")
        W32M_POSTFIX("win32metadata:optional"),
    HANDLE* reserved W32M_POSTFIX("win32metadata:reserved"),
    void** com
        W32M_POSTFIX("win32metadata:out")
        W32M_POSTFIX("win32metadata:optional")
        W32M_POSTFIX("win32metadata:com_out_ptr"),
    BOOL* retval
        W32M_POSTFIX("win32metadata:out")
        W32M_POSTFIX("win32metadata:retval"));

BOOL UsesCallback(INTERNAL_CALLBACK callback);

BOOL PostfixAnnotatedFunction(
    HANDLE* result
        W32M_POSTFIX("win32metadata:invalid_handle=-0x1L")
        W32M_POSTFIX("win32metadata:invalid_handle=0x0UL")
        W32M_POSTFIX("win32metadata:raii_free=CloseHandle"),
    HANDLE** reduced
        W32M_POSTFIX("win32metadata:reduce_pointer_level"));

HANDLE WINAPI TrailingAnnotatedReturn(void)
    W32M_POSTFIX("win32metadata:invalid_handle=-1")
    W32M_POSTFIX("win32metadata:invalid_handle=0")
    W32M_POSTFIX("win32metadata:raii_free=CloseHandle");

char* WINAPI AnnotatedStringReturn(void)
    W32M_POSTFIX("win32metadata:free_with=LocalFree")
    W32M_POSTFIX("win32metadata:do_not_release")
    W32M_POSTFIX("win32metadata:not_null_terminated");

W32M("win32metadata:raii_free=CloseHandle")
W32M("win32metadata:invalid_handle=-1")
HANDLE AnnotatedReturn(void);

typedef struct
    W32M_POSTFIX("win32metadata:struct_size_field=cbSize")
    W32M_POSTFIX("win32metadata:native_inheritance=BASE_STRUCT")
    W32M_POSTFIX("win32metadata:supported_os=windows10.0.10240")
    ANNOTATED_STRUCT {
    DWORD cbSize;
    W32M("win32metadata:associated_enum=FLAGS")
    DWORD flags;
    char* text
        W32M_POSTFIX("win32metadata:not_null_terminated")
        W32M_POSTFIX("win32metadata:null_null_terminated")
        W32M_POSTFIX("win32metadata:native_encoding=ansi");
    HANDLE* allocated
        W32M_POSTFIX("win32metadata:free_with=CloseHandle");
    BYTE values[16]
        W32M_POSTFIX("win32metadata:array_count_field=cbSize");
    HANDLE** callback
        W32M_POSTFIX("win32metadata:reduce_pointer_level");
} ANNOTATED_STRUCT;
