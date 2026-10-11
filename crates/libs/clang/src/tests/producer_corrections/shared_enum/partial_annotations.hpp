#include "partial.hpp"
struct AnnotatedRecord {
    __attribute__((annotate("win32metadata:associated_enum=SHARED_CURVE"))) unsigned value;
};
extern "C" {
void AnnotatedRecordFunction(AnnotatedRecord value);
void AnnotatedFunction(
    __attribute__((annotate("win32metadata:associated_enum=SHARED_CURVE"))) unsigned value);
}
