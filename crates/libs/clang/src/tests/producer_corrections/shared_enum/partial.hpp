#include "a/Curve.h"
struct BrokenRecord {
    SHARED_CURVE curve;
    int independent_field;
};
struct TransitiveRecord {
    BrokenRecord record;
};
struct GoodRecord {
    int value;
};
typedef void (__stdcall* BrokenCallback)(SHARED_CURVE curve);
struct __declspec(uuid("5737fab7-85b0-4a59-a3ed-efdfdbd021e8")) BrokenInterface {
    virtual int __stdcall UseCurve(SHARED_CURVE curve) = 0;
    virtual int __stdcall IndependentMethod(int value) = 0;
};
struct __declspec(uuid("88bdaf46-758b-4195-97c5-cc0a8b40a6cd")) TransitiveInterface
    : BrokenInterface {
    virtual int __stdcall ExtraMethod(int value) = 0;
};
extern "C" {
int IndependentFunction(int value);
void DirectEnumFunction(SHARED_CURVE curve);
void DirectRecordFunction(BrokenRecord record);
void TransitiveFunction(TransitiveRecord* record);
void InterfaceFunction(BrokenInterface* value);
void TransitiveInterfaceFunction(TransitiveInterface* value);
void CallbackFunction(BrokenCallback value);
}
