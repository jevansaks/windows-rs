#pragma once

#ifndef FIELD_KIND
#define FIELD_KIND long
#endif
#ifndef REF_CV
#define REF_CV const
#endif
#ifndef REF_OPERATOR
#define REF_OPERATOR &
#endif

struct ReferencePayload {
    FIELD_KIND value;
    unsigned short low;
    unsigned short high;
    unsigned char bytes[8];
};

typedef REF_CV FIELD_KIND REF_OPERATOR PrimitiveReference;
typedef REF_CV ReferencePayload REF_OPERATOR PayloadReference;
