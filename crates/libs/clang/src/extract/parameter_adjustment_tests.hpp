#pragma once

#if defined(PARAMETER_ALTERNATIVE)
#define PARAMETER_VALUE unsigned int
#else
#define PARAMETER_VALUE int
#endif

typedef struct Element { PARAMETER_VALUE value; } Element;
typedef struct Info {
    Element element;
    unsigned int locale;
    unsigned short description[8];
} Info;

#if defined(PARAMETER_ARRAYS)
extern Element noncallable_incomplete[];
#define PARAMETER_INPUT(count) __attribute__((annotate("_In_reads_(" #count ")")))
struct __declspec(uuid("ab120000-0000-0000-0000-000000000001")) ArrayProbe {
    virtual int __stdcall Elements(
        unsigned int count, PARAMETER_INPUT(count) const Element values[],
        unsigned int other, PARAMETER_INPUT(other) const Element more[]) = 0;
    virtual int __stdcall Infos(unsigned int count, Info infos[]) = 0;
    virtual int __stdcall Fixed(unsigned int count, const Element values[2]) = 0;
    virtual int __stdcall Primitive(unsigned int count, const int values[]) = 0;
    virtual int __stdcall InnerTwo(unsigned int count, const Element values[][2]) = 0;
    virtual int __stdcall InnerThree(unsigned int count, const Element values[][3]) = 0;
    virtual int __stdcall PointerElements(
        unsigned int count, PARAMETER_INPUT(count) const Element *values,
        unsigned int other, PARAMETER_INPUT(other) const Element *more) = 0;
    virtual int __stdcall PointerInfos(unsigned int count, Info *infos) = 0;
    virtual int __stdcall PointerFixed(unsigned int count, const Element *values) = 0;
    virtual int __stdcall PointerPrimitive(unsigned int count, const int *values) = 0;
    virtual int __stdcall PointerInnerTwo(unsigned int count, const Element (*values)[2]) = 0;
    virtual int __stdcall PointerInnerThree(unsigned int count, const Element (*values)[3]) = 0;
};
static_assert(__is_same(decltype(&ArrayProbe::Elements), decltype(&ArrayProbe::PointerElements)));
static_assert(__is_same(decltype(&ArrayProbe::Infos), decltype(&ArrayProbe::PointerInfos)));
static_assert(__is_same(decltype(&ArrayProbe::Fixed), decltype(&ArrayProbe::PointerFixed)));
static_assert(__is_same(decltype(&ArrayProbe::Primitive), decltype(&ArrayProbe::PointerPrimitive)));
static_assert(__is_same(decltype(&ArrayProbe::InnerTwo), decltype(&ArrayProbe::PointerInnerTwo)));
static_assert(__is_same(decltype(&ArrayProbe::InnerThree), decltype(&ArrayProbe::PointerInnerThree)));
using MutableElementParameter = int (__stdcall ArrayProbe::*)(unsigned int, Element *);
static_assert(!__is_same(decltype(&ArrayProbe::Fixed), MutableElementParameter));
static_assert(!__is_same(decltype(&ArrayProbe::InnerTwo), decltype(&ArrayProbe::InnerThree)));
#elif defined(PARAMETER_ALIASES)
typedef Element Items[2];
struct __declspec(uuid("ab120000-0000-0000-0000-000000000004")) AliasProbe {
    virtual int __stdcall Plain(Items values) = 0;
    virtual int __stdcall Const(const Items values) = 0;
    virtual int __stdcall PointerPlain(Element *values) = 0;
    virtual int __stdcall PointerConst(const Element *values) = 0;
    virtual int __stdcall PointerOther(const Info *values) = 0;
};
static_assert(__is_same(decltype(&AliasProbe::Plain), decltype(&AliasProbe::PointerPlain)));
static_assert(__is_same(decltype(&AliasProbe::Const), decltype(&AliasProbe::PointerConst)));
static_assert(!__is_same(decltype(&AliasProbe::Plain), decltype(&AliasProbe::Const)));
#elif defined(PARAMETER_ATTRIBUTES)
typedef Element *NoDeref __attribute__((noderef));
struct __declspec(uuid("ab120000-0000-0000-0000-000000000003")) AttributeProbe {
    virtual int __stdcall Sal(Element *value __attribute__((annotate("_In_")))) = 0;
    virtual int __stdcall RejectTypeUse(NoDeref value) = 0;
    virtual int __stdcall RejectDeclaration(
        Element *const value __attribute__((pass_object_size(0)))) = 0;
};
#else
struct __declspec(uuid("ab120000-0000-0000-0000-000000000002")) PointerProbe {
    virtual int __stdcall Elements(
        unsigned int count, const Element *values,
        unsigned int other, const Element *more) = 0;
    virtual int __stdcall Infos(unsigned int count, Info *infos) = 0;
    virtual int __stdcall Primitive(unsigned int count, const int *values) = 0;
    virtual int __stdcall Sal(Element *value __attribute__((annotate("_In_")))) = 0;
};
#endif
