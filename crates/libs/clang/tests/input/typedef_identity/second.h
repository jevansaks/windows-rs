#pragma once
#include "shared.h"
struct SECOND_RECORD {
    PVOID value;
    LPVOID const *boundary;
    HANDLE handle;
    UNLISTED other;
};
extern "C" void Second(LPVOID value);
extern "C" void Raw(void *value);
extern "C" void ProjectionControls(
    __attribute__((annotate("_Out_opt_")))
    __attribute__((annotate("_Out_writes_bytes_to_(size, size)"))) LPVOID value,
    PVOID plain,
    HANDLE handle,
    UNLISTED other,
    LPVOID *nested,
    LPVOID const *boundary,
    PVOID const *pvoidBoundary,
    void *raw,
    unsigned int size,
    __attribute__((annotate("_In_"))) PCOR_SIGNATURE signature,
    PCCOR_SIGNATURE *signatureBoundary);
