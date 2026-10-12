#pragma once
#ifndef OWN_EXCEPTION
#define OWN_EXCEPTION
#endif
typedef BASE_VALUE Status;
struct __declspec(uuid("c50e1b53-a52f-4438-b4ad-7a62421fe761")) GenericBase {
    virtual Status __stdcall First(Status* value) = 0;
    virtual int __stdcall Second() = 0;
};
struct __declspec(uuid("085b9571-402b-4397-8f50-1d56c107bb65")) GenericDerived : GenericBase {
    virtual int __stdcall Own() OWN_EXCEPTION = 0;
};
