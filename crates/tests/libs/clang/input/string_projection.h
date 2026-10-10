//! args -x c++ -fms-extensions --target=x86_64-pc-windows-msvc
//! reference-default
#define SAL(text) __attribute__((annotate(text)))
#define META(text) __attribute__((annotate("win32metadata:" text)))
typedef char CHAR;
typedef unsigned short WCHAR;
typedef CHAR* PSTR;
typedef const CHAR* PCSTR;
typedef WCHAR* PWSTR;
typedef const WCHAR* PCWSTR;
typedef const CHAR* PCCH;
typedef const WCHAR* PCWCH;
typedef CHAR* LPSTR;
typedef const CHAR* LPCSTR;
typedef WCHAR* LPWSTR;
typedef const WCHAR* LPCWSTR;
typedef CHAR* PZZSTR;
typedef const CHAR* PCZZSTR;
typedef WCHAR* PZZWSTR;
typedef const WCHAR* PCZZWSTR;
typedef unsigned char BYTE;
typedef void* LPVOID;
typedef void* PVOID;
typedef void* HANDLE;
typedef CHAR* TEXT_HANDLE;
typedef PCSTR TEXT_HANDLE_ANSI;
typedef PCWSTR TEXT_HANDLE_W;

struct NominalStrings {
    TEXT_HANDLE_ANSI narrow;
    TEXT_HANDLE_W wide;
};
extern "C" void UseNominalStrings(
    TEXT_HANDLE_ANSI narrow,
    TEXT_HANDLE_W wide,
    SAL("_In_") TEXT_HANDLE_ANSI input,
    SAL("_In_") TEXT_HANDLE_W input_wide,
    SAL("_In_z_") TEXT_HANDLE_ANSI terminated,
    SAL("_In_z_") TEXT_HANDLE_W terminated_wide);
extern "C" TEXT_HANDLE_ANSI GetNominalString();
extern "C" TEXT_HANDLE_W GetNominalWideString();

#define STRING_CAPACITY 8
extern "C" void ConstantCounts(
    SAL("_In_reads_(STRING_CAPACITY)") PCWSTR elements,
    SAL("_In_reads_bytes_(STRING_CAPACITY)") PCWSTR bytes,
    SAL("_In_reads_(STRING_CAPACITY + 2)") PCSTR expression);
#undef STRING_CAPACITY
#define STRING_CAPACITY 12
extern "C" void AlteredConstantCounts(
    SAL("_In_reads_(STRING_CAPACITY)") PCWSTR elements,
    SAL("_In_reads_bytes_(STRING_CAPACITY)") PCWSTR bytes);

struct Strings {
    PCSTR input;
    PWSTR output;
    META("not_null_terminated") PCWSTR negative;
    META("null_null_terminated") PCSTR multi;
    PCWSTR* nested;
    PWSTR const* outer_const;
    const WCHAR* raw;
};
struct CountedFields {
    unsigned length;
    META("array_count_field=length") PCSTR text;
    META("not_null_terminated") PWSTR wide;
    META("const") PCSTR already_const;
    struct {
        PCSTR text;
        META("not_null_terminated") PWSTR buffer;
    } inner;
    PCSTR array[2];
};
struct __declspec(uuid("12345678-1234-1234-1234-123456789abc")) IStrings {
    virtual PCWSTR Read(PCSTR value) = 0;
    virtual PSTR Write(SAL("_Out_z_") CHAR* value) = 0;
};
typedef PCWSTR (__stdcall *StringCallback)(
    SAL("_In_z_") const CHAR* input,
    SAL("_Out_z_") WCHAR* output);
extern "C" PCSTR GetString();
META("null_null_terminated") extern "C" PCWSTR GetMulti();
extern "C" void UseStrings(
    SAL("_In_z_") const CHAR* input,
    SAL("_Out_z_") WCHAR* output,
    SAL("_In_opt_z_") const WCHAR* optional,
    unsigned count,
    SAL("_In_reads_(count)") PCSTR counted,
    SAL("_In_reads_bytes_(count)") PSTR bytes,
    SAL("_In_reads_(count)") const CHAR* binary,
    SAL("_NullNull_terminated_") const WCHAR* multi,
    META("not_null_terminated") PCSTR negative,
    PCCH narrow_buffer,
    PCWCH wide_buffer,
    PCWSTR* nested,
    StringCallback callback);

extern "C" PSTR GetMutable();
extern "C" PCSTR const* GetNested();
META("null_null_terminated") extern "C" const WCHAR* GetRawMulti();
extern "C" void PlainRaw(const WCHAR* value);
extern "C" void MultiRaw(META("null_null_terminated") const WCHAR* value);
extern "C" void MoreStrings(
    SAL("_In_z_") CHAR* mutable_input,
    SAL("_Out_z_") CHAR* mutable_output,
    SAL("_Out_opt_z_") CHAR* optional_output,
    SAL("_In_") LPSTR declared_mutable,
    LPCSTR legacy_const,
    LPWSTR legacy_wide,
    LPCWSTR legacy_wide_const,
    unsigned count,
    SAL("_Inout_updates_(count)") PWSTR counted_inout,
    SAL("_In_reads_bytes_(count)") const BYTE* binary,
    SAL("_In_z_") META("not_null_terminated") const CHAR* explicit_negative,
    SAL("_In_reads_or_z_(count)") const WCHAR* counted_or_z,
    PZZSTR multi_ansi,
    PCZZSTR const_multi_ansi,
    PZZWSTR multi_wide,
    PCZZWSTR const_multi_wide,
    SAL("_In_reads_(4)") PCWSTR constant_count);
extern "C" void PointerGates(
    LPVOID ordinary,
    SAL("_In_") PVOID input,
    HANDLE handle,
    LPVOID const* nested,
    TEXT_HANDLE nominal);
