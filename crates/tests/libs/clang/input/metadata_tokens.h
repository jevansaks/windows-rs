typedef unsigned long mdToken;
typedef mdToken mdFieldDef;
typedef mdToken mdMemberRef;

typedef struct COR_FIELD_OFFSET
{
    mdFieldDef ridOfField;
    unsigned long offset;
} COR_FIELD_OFFSET;

typedef struct COR_SECATTR
{
    mdMemberRef constructor;
} COR_SECATTR;
