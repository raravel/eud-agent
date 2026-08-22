#include "MapAgentJson.h"

#include <charconv>
#include <cmath>
#include <limits>

namespace mapagent {
namespace {

class JsonParser final
{
public:
    JsonParser(std::string text, std::string context) : text_(std::move(text)), context_(std::move(context)) {}

    Json parse()
    {
        skipWhitespace();
        Json result = parseValue();
        skipWhitespace();
        if ( position_ != text_.size() )
            error("unexpected trailing JSON content");
        return result;
    }

private:
    std::string text_;
    std::string context_;
    std::size_t position_ = 0;

    [[noreturn]] void error(const std::string& message) const
    {
        std::size_t line = 1;
        std::size_t column = 1;
        for ( std::size_t index = 0; index < position_ && index < text_.size(); ++index )
        {
            if ( text_[index] == '\n' )
            {
                ++line;
                column = 1;
            }
            else
                ++column;
        }
        throw JsonError(context_ + ":" + std::to_string(line) + ":" + std::to_string(column) + ": " + message);
    }

    void skipWhitespace()
    {
        while ( position_ < text_.size() &&
                (text_[position_] == ' ' || text_[position_] == '\t' ||
                 text_[position_] == '\r' || text_[position_] == '\n') )
            ++position_;
    }

    bool consume(char expected)
    {
        if ( position_ < text_.size() && text_[position_] == expected )
        {
            ++position_;
            return true;
        }
        return false;
    }

    void expect(char expected, const char* message)
    {
        if ( !consume(expected) )
            error(message);
    }

    Json parseValue()
    {
        if ( position_ >= text_.size() )
            error("unexpected end of JSON");
        const char c = text_[position_];
        if ( c == '{' ) return Json(parseObject());
        if ( c == '[' ) return Json(parseArray());
        if ( c == '"' ) return Json(parseString());
        if ( c == '-' || (c >= '0' && c <= '9') ) return parseNumber();
        if ( text_.compare(position_, 4, "true") == 0 )
        {
            position_ += 4;
            return Json(true);
        }
        if ( text_.compare(position_, 5, "false") == 0 )
        {
            position_ += 5;
            return Json(false);
        }
        if ( text_.compare(position_, 4, "null") == 0 )
        {
            position_ += 4;
            return Json(nullptr);
        }
        error("invalid JSON value");
    }

    Json::Object parseObject()
    {
        expect('{', "expected '{'");
        skipWhitespace();
        Json::Object object;
        if ( consume('}') )
            return object;
        while ( true )
        {
            if ( position_ >= text_.size() || text_[position_] != '"' )
                error("object key must be a string");
            std::string key = parseString();
            if ( object.find(key) != object.end() )
                error("duplicate object key '" + key + "'");
            skipWhitespace();
            expect(':', "expected ':' after object key");
            skipWhitespace();
            object.emplace(std::move(key), parseValue());
            skipWhitespace();
            if ( consume('}') )
                break;
            expect(',', "expected ',' or '}' in object");
            skipWhitespace();
        }
        return object;
    }

    Json::Array parseArray()
    {
        expect('[', "expected '['");
        skipWhitespace();
        Json::Array array;
        if ( consume(']') )
            return array;
        while ( true )
        {
            array.push_back(parseValue());
            skipWhitespace();
            if ( consume(']') )
                break;
            expect(',', "expected ',' or ']' in array");
            skipWhitespace();
        }
        return array;
    }

    static int hexDigit(char c)
    {
        if ( c >= '0' && c <= '9' ) return c - '0';
        if ( c >= 'a' && c <= 'f' ) return c - 'a' + 10;
        if ( c >= 'A' && c <= 'F' ) return c - 'A' + 10;
        return -1;
    }

    std::uint16_t parseHex4()
    {
        if ( position_ + 4 > text_.size() )
            error("truncated Unicode escape");
        std::uint16_t value = 0;
        for ( int index = 0; index < 4; ++index )
        {
            const int digit = hexDigit(text_[position_++]);
            if ( digit < 0 )
                error("invalid Unicode escape");
            value = static_cast<std::uint16_t>((value << 4) | digit);
        }
        return value;
    }

    static void appendUtf8(std::string& output, std::uint32_t codepoint)
    {
        if ( codepoint <= 0x7f )
            output.push_back(static_cast<char>(codepoint));
        else if ( codepoint <= 0x7ff )
        {
            output.push_back(static_cast<char>(0xc0 | (codepoint >> 6)));
            output.push_back(static_cast<char>(0x80 | (codepoint & 0x3f)));
        }
        else if ( codepoint <= 0xffff )
        {
            output.push_back(static_cast<char>(0xe0 | (codepoint >> 12)));
            output.push_back(static_cast<char>(0x80 | ((codepoint >> 6) & 0x3f)));
            output.push_back(static_cast<char>(0x80 | (codepoint & 0x3f)));
        }
        else
        {
            output.push_back(static_cast<char>(0xf0 | (codepoint >> 18)));
            output.push_back(static_cast<char>(0x80 | ((codepoint >> 12) & 0x3f)));
            output.push_back(static_cast<char>(0x80 | ((codepoint >> 6) & 0x3f)));
            output.push_back(static_cast<char>(0x80 | (codepoint & 0x3f)));
        }
    }

    std::string parseString()
    {
        expect('"', "expected string");
        std::string output;
        while ( position_ < text_.size() )
        {
            const unsigned char c = static_cast<unsigned char>(text_[position_++]);
            if ( c == '"' )
                return output;
            if ( c < 0x20 )
                error("unescaped control character in string");
            if ( c != '\\' )
            {
                output.push_back(static_cast<char>(c));
                continue;
            }
            if ( position_ >= text_.size() )
                error("truncated string escape");
            const char escape = text_[position_++];
            switch ( escape )
            {
            case '"': output.push_back('"'); break;
            case '\\': output.push_back('\\'); break;
            case '/': output.push_back('/'); break;
            case 'b': output.push_back('\b'); break;
            case 'f': output.push_back('\f'); break;
            case 'n': output.push_back('\n'); break;
            case 'r': output.push_back('\r'); break;
            case 't': output.push_back('\t'); break;
            case 'u':
            {
                std::uint32_t codepoint = parseHex4();
                if ( codepoint >= 0xd800 && codepoint <= 0xdbff )
                {
                    if ( position_ + 2 > text_.size() || text_[position_] != '\\' || text_[position_ + 1] != 'u' )
                        error("high surrogate without low surrogate");
                    position_ += 2;
                    const std::uint16_t low = parseHex4();
                    if ( low < 0xdc00 || low > 0xdfff )
                        error("invalid low surrogate");
                    codepoint = 0x10000 + ((codepoint - 0xd800) << 10) + (low - 0xdc00);
                }
                else if ( codepoint >= 0xdc00 && codepoint <= 0xdfff )
                    error("low surrogate without high surrogate");
                appendUtf8(output, codepoint);
                break;
            }
            default: error("invalid string escape");
            }
        }
        error("unterminated string");
    }

    Json parseNumber()
    {
        const std::size_t start = position_;
        if ( consume('-') && position_ >= text_.size() )
            error("truncated number");
        if ( position_ < text_.size() && text_[position_] == '0' )
        {
            ++position_;
            if ( position_ < text_.size() && text_[position_] >= '0' && text_[position_] <= '9' )
                error("leading zero in number");
        }
        else
        {
            const std::size_t digits = position_;
            while ( position_ < text_.size() && text_[position_] >= '0' && text_[position_] <= '9' )
                ++position_;
            if ( digits == position_ )
                error("number requires digits");
        }
        bool real = false;
        if ( consume('.') )
        {
            real = true;
            const std::size_t digits = position_;
            while ( position_ < text_.size() && text_[position_] >= '0' && text_[position_] <= '9' )
                ++position_;
            if ( digits == position_ )
                error("fraction requires digits");
        }
        if ( position_ < text_.size() && (text_[position_] == 'e' || text_[position_] == 'E') )
        {
            real = true;
            ++position_;
            if ( position_ < text_.size() && (text_[position_] == '+' || text_[position_] == '-') )
                ++position_;
            const std::size_t digits = position_;
            while ( position_ < text_.size() && text_[position_] >= '0' && text_[position_] <= '9' )
                ++position_;
            if ( digits == position_ )
                error("exponent requires digits");
        }
        if ( !real )
        {
            std::int64_t value = 0;
            const auto result = std::from_chars(text_.data() + start, text_.data() + position_, value, 10);
            if ( result.ec != std::errc() || result.ptr != text_.data() + position_ )
                error("integer is out of range");
            return Json(value);
        }
        double value = 0;
        const auto result = std::from_chars(text_.data() + start, text_.data() + position_, value, std::chars_format::general);
        if ( result.ec != std::errc() || result.ptr != text_.data() + position_ || !std::isfinite(value) )
            error("number is out of range");
        return Json(value);
    }
};

void appendEscaped(std::string& output, std::string_view value)
{
    static constexpr char Hex[] = "0123456789abcdef";
    output.push_back('"');
    for ( const unsigned char c : value )
    {
        switch ( c )
        {
        case '"': output += "\\\""; break;
        case '\\': output += "\\\\"; break;
        case '\b': output += "\\b"; break;
        case '\f': output += "\\f"; break;
        case '\n': output += "\\n"; break;
        case '\r': output += "\\r"; break;
        case '\t': output += "\\t"; break;
        default:
            if ( c < 0x20 )
            {
                output += "\\u00";
                output.push_back(Hex[c >> 4]);
                output.push_back(Hex[c & 0x0f]);
            }
            else
                output.push_back(static_cast<char>(c));
        }
    }
    output.push_back('"');
}

void appendJson(std::string& output, const Json& value)
{
    if ( std::holds_alternative<std::nullptr_t>(value.value) )
        output += "null";
    else if ( const auto* boolean = std::get_if<bool>(&value.value) )
        output += *boolean ? "true" : "false";
    else if ( const auto* integer = std::get_if<std::int64_t>(&value.value) )
        output += std::to_string(*integer);
    else if ( const auto* real = std::get_if<double>(&value.value) )
    {
        if ( !std::isfinite(*real) )
            throw JsonError("cannot serialize non-finite JSON number");
        char buffer[64];
        const auto result = std::to_chars(buffer, buffer + sizeof(buffer), *real, std::chars_format::general,
            std::numeric_limits<double>::max_digits10);
        if ( result.ec != std::errc() )
            throw JsonError("cannot serialize JSON number");
        output.append(buffer, result.ptr);
    }
    else if ( const auto* string = std::get_if<std::string>(&value.value) )
        appendEscaped(output, *string);
    else if ( const auto* array = std::get_if<Json::Array>(&value.value) )
    {
        output.push_back('[');
        for ( std::size_t index = 0; index < array->size(); ++index )
        {
            if ( index != 0 ) output.push_back(',');
            appendJson(output, (*array)[index]);
        }
        output.push_back(']');
    }
    else
    {
        const auto& object = std::get<Json::Object>(value.value);
        output.push_back('{');
        std::size_t index = 0;
        for ( const auto& entry : object )
        {
            if ( index++ != 0 ) output.push_back(',');
            appendEscaped(output, entry.first);
            output.push_back(':');
            appendJson(output, entry.second);
        }
        output.push_back('}');
    }
}

} // namespace

Json parseJson(std::string text, std::string context)
{
    return JsonParser(std::move(text), std::move(context)).parse();
}

std::string serializeJson(const Json& value)
{
    std::string output;
    appendJson(output, value);
    return output;
}

const Json::Object& objectValue(const Json& value, const std::string& context)
{
    const auto* object = std::get_if<Json::Object>(&value.value);
    if ( object == nullptr ) throw JsonError(context + ": expected an object");
    return *object;
}

const Json::Array& arrayValue(const Json& value, const std::string& context)
{
    const auto* array = std::get_if<Json::Array>(&value.value);
    if ( array == nullptr ) throw JsonError(context + ": expected an array");
    return *array;
}

const std::string& stringValue(const Json& value, const std::string& context)
{
    const auto* string = std::get_if<std::string>(&value.value);
    if ( string == nullptr ) throw JsonError(context + ": expected a string");
    return *string;
}

std::int64_t integerValue(const Json& value, const std::string& context)
{
    const auto* integer = std::get_if<std::int64_t>(&value.value);
    if ( integer == nullptr ) throw JsonError(context + ": expected an integer");
    return *integer;
}

std::size_t sizeValue(const Json& value, const std::string& context, bool positive)
{
    const std::int64_t integer = integerValue(value, context);
    if ( integer < 0 || (positive && integer == 0) )
        throw JsonError(context + (positive ? ": expected a positive integer" : ": expected a non-negative integer"));
    if ( static_cast<std::uint64_t>(integer) > std::numeric_limits<std::size_t>::max() )
        throw JsonError(context + ": integer is too large");
    return static_cast<std::size_t>(integer);
}

bool boolValue(const Json& value, const std::string& context)
{
    const auto* boolean = std::get_if<bool>(&value.value);
    if ( boolean == nullptr ) throw JsonError(context + ": expected a boolean");
    return *boolean;
}

const Json& requiredField(const Json::Object& object, const std::string& name, const std::string& context)
{
    const auto found = object.find(name);
    if ( found == object.end() ) throw JsonError(context + ": missing field '" + name + "'");
    return found->second;
}

const Json* optionalField(const Json::Object& object, const std::string& name) noexcept
{
    const auto found = object.find(name);
    return found == object.end() ? nullptr : &found->second;
}

void exactFields(const Json::Object& object, const std::set<std::string>& expected, const std::string& context)
{
    allowedFields(object, expected, context);
    for ( const auto& name : expected )
    {
        if ( object.find(name) == object.end() )
            throw JsonError(context + ": missing field '" + name + "'");
    }
}

void allowedFields(const Json::Object& object, const std::set<std::string>& allowed, const std::string& context)
{
    for ( const auto& entry : object )
    {
        if ( allowed.find(entry.first) == allowed.end() )
            throw JsonError(context + ": unknown field '" + entry.first + "'");
    }
}

} // namespace mapagent
