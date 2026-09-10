typedef unsigned long mdToken;
typedef mdToken mdFieldDef;
typedef mdToken mdMemberRef;
typedef unsigned long SCRIPTTHREADID;

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
} ACTIVE_SCRIPT_THREAD;
