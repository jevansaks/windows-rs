typedef unsigned long mdToken;
typedef mdToken mdFieldDef;
typedef mdToken mdMemberRef;
typedef unsigned long SCRIPTTHREADID;
typedef unsigned char COR_SIGNATURE;
typedef COR_SIGNATURE* PCOR_SIGNATURE;
typedef const COR_SIGNATURE* PCCOR_SIGNATURE;
typedef void* HCORENUM;
typedef const char* MDUTF8CSTR;

typedef struct COR_FIELD_OFFSET
{
    mdFieldDef ridOfField;
    unsigned long offset;
} COR_FIELD_OFFSET;

typedef struct COR_SECATTR
{
    mdMemberRef constructor;
} COR_SECATTR;

typedef struct ACTIVE_SCRIPT_THREAD
{
    SCRIPTTHREADID id;
    PCOR_SIGNATURE mutable_signature;
    PCCOR_SIGNATURE signature;
    HCORENUM enumerator;
    MDUTF8CSTR name;
} ACTIVE_SCRIPT_THREAD;

typedef struct AE_ACLMOD
{
    unsigned long action;
} AE_ACLMOD;

#define AE_ACLMOD 12
