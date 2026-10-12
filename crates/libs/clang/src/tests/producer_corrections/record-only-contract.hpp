#pragma once
#include "nominal-contract.hpp"
struct __declspec(uuid("b2ed46df-42e3-46d9-bc8b-a57d473bc5dd")) RecordOnlyContract {
    virtual PlainAlias __stdcall Transform(PartsAlias parts, ScopedAlias scoped) = 0;
};
