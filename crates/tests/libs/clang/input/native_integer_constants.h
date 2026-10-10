//! args -x c++ --target=x86_64-pc-windows-msvc

typedef unsigned long DWORD;

#define PROC_THREAD_ATTRIBUTE_PARENT_PROCESS 0x00020000
#define MakeProcThreadAttributeConst(Attribute) \
    const DWORD __forceconst__##Attribute = Attribute;

MakeProcThreadAttributeConst(PROC_THREAD_ATTRIBUTE_PARENT_PROCESS)
static_assert(__forceconst__PROC_THREAD_ATTRIBUTE_PARENT_PROCESS == 0x20000UL);

const DWORD HIGH_BIT = 0x80000000UL;
const unsigned long long WIDE_HIGH_BIT = 0x8000000000000000ULL;
const unsigned char BYTE_VALUE = 255;
const unsigned short WORD_VALUE = 65535;
const signed char SIGNED_BYTE = -128;
const short SIGNED_WORD = -32768;
const int NEGATIVE = -17;
const long long SIGNED_WIDE = -9223372036854775807LL - 1;
const DWORD NATIVE_EXPRESSION = (3UL << 20) | 17UL;
constexpr DWORD CONSTEXPR_VALUE = NATIVE_EXPRESSION + 2UL;
const bool BOOL_TRUE = true;
const bool BOOL_FALSE = false;

enum NativeFlags : unsigned long { NativeFlag = 1 };
const DWORD ANNOTATED
    __attribute__((annotate("win32metadata:associated_enum=NativeFlags"))) = 0x80000000UL;

int runtime_value();
const int NONCONSTANT = runtime_value();
int MUTABLE = 42;
extern const int DECLARATION_ONLY;
const char* const UNSUPPORTED_POINTER = "value";

const float FLOAT_VALUE = 1.25f;
const double DOUBLE_VALUE = -2.5;

#include "native_integer_constants_unselected.hpp"
