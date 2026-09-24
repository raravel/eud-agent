/* Non-MSVC only (compiled by crates/isom-sys/build.rs, not MappingCoreLib.vcxproj).
 *
 * EscapeStrings.cpp defines each convertStr<In, Out> as a separate overload
 * template with concrete parameter types. MSVC mangles a function template
 * specialization by its resulting types, so those symbols satisfy callers of the
 * header's primary template `convertStr(const In &, Out &)`. The Itanium C++ ABI
 * mangles the primary template's dependent signature instead, so callers there
 * need explicit specializations of the primary template itself. This TU only sees
 * the header declaration, which keeps the specializations unambiguous. Bodies
 * mirror EscapeStrings.cpp.
 */
#include "EscapeStrings.h"

bool MakeOneLineChkdStr(const std::string & inRawString, size_t inRawStringLength, ChkdString & outChkdString);

template <> void convertStr<RawString, RawString>(const RawString & inString, RawString & outString)
{
    outString = inString;
}

template <> void convertStr<RawString, EscString>(const RawString & inString, EscString & outString)
{
    makeEscStr(inString, outString);
}

template <> void convertStr<RawString, ChkdString>(const RawString & inString, ChkdString & outString)
{
    makeChkdStr(inString, outString);
}

template <> void convertStr<RawString, SingleLineChkdString>(const RawString & inString, SingleLineChkdString & outString)
{
    MakeOneLineChkdStr(inString, inString.length(), outString);
}

template <> void convertStr<EscString, RawString>(const EscString & inString, RawString & outString)
{
    parseEscStr(inString, outString);
}

template <> void convertStr<EscString, EscString>(const EscString & inString, EscString & outString)
{
    outString = inString;
}

template <> void convertStr<EscString, ChkdString>(const EscString & inString, ChkdString & outString)
{
    RawString rawString;
    parseEscStr(inString, rawString);
    makeChkdStr(rawString, outString);
}

template <> void convertStr<EscString, SingleLineChkdString>(const EscString & inString, SingleLineChkdString & outString)
{
    RawString rawString;
    parseEscStr(inString, rawString);
    MakeOneLineChkdStr(rawString, rawString.length(), outString);
}

template <> void convertStr<ChkdString, RawString>(const ChkdString & inString, RawString & outString)
{
    parseChkdStr(inString, outString);
}

template <> void convertStr<ChkdString, EscString>(const ChkdString & inString, EscString & outString)
{
    RawString rawString;
    parseChkdStr(inString, rawString);
    makeEscStr(rawString, outString);
}

template <> void convertStr<ChkdString, ChkdString>(const ChkdString & inString, ChkdString & outString)
{
    outString = inString;
}

template <> void convertStr<ChkdString, SingleLineChkdString>(const ChkdString & inString, SingleLineChkdString & outString)
{
    RawString rawString;
    parseChkdStr(inString, rawString);
    MakeOneLineChkdStr(rawString, rawString.length(), outString);
}

template <> void convertStr<SingleLineChkdString, RawString>(const SingleLineChkdString & inString, RawString & outString)
{
    parseChkdStr(inString, outString);
}

template <> void convertStr<SingleLineChkdString, EscString>(const SingleLineChkdString & inString, EscString & outString)
{
    RawString rawString;
    parseChkdStr(inString, rawString);
    makeEscStr(rawString, outString);
}

template <> void convertStr<SingleLineChkdString, ChkdString>(const SingleLineChkdString & inString, ChkdString & outString)
{
    RawString rawString;
    parseChkdStr(inString, rawString);
    makeChkdStr(rawString, outString);
}

template <> void convertStr<SingleLineChkdString, SingleLineChkdString>(const SingleLineChkdString & inString, SingleLineChkdString & outString)
{
    outString = inString;
}
