#pragma once

#include <cstdint>
#include <map>
#include <set>
#include <stdexcept>
#include <string>
#include <string_view>
#include <variant>
#include <vector>

namespace mapagent {

class JsonError final : public std::runtime_error {
public:
    explicit JsonError(std::string message) : std::runtime_error(std::move(message)) {}
};

struct Json {
    using Array = std::vector<Json>;
    using Object = std::map<std::string, Json>;
    using Value = std::variant<std::nullptr_t, bool, std::int64_t, double, std::string, Array, Object>;

    Value value{nullptr};

    Json() = default;
    Json(std::nullptr_t) : value(nullptr) {}
    Json(bool input) : value(input) {}
    Json(std::int64_t input) : value(input) {}
    Json(std::size_t input) : value(static_cast<std::int64_t>(input)) {}
    Json(double input) : value(input) {}
    Json(const char* input) : value(std::string(input)) {}
    Json(std::string input) : value(std::move(input)) {}
    Json(Array input) : value(std::move(input)) {}
    Json(Object input) : value(std::move(input)) {}
};

Json parseJson(std::string text, std::string context);
std::string serializeJson(const Json& value);

const Json::Object& objectValue(const Json& value, const std::string& context);
const Json::Array& arrayValue(const Json& value, const std::string& context);
const std::string& stringValue(const Json& value, const std::string& context);
std::int64_t integerValue(const Json& value, const std::string& context);
std::size_t sizeValue(const Json& value, const std::string& context, bool positive = false);
bool boolValue(const Json& value, const std::string& context);
const Json& requiredField(const Json::Object& object, const std::string& name, const std::string& context);
const Json* optionalField(const Json::Object& object, const std::string& name) noexcept;
void exactFields(const Json::Object& object, const std::set<std::string>& expected, const std::string& context);
void allowedFields(const Json::Object& object, const std::set<std::string>& allowed, const std::string& context);

} // namespace mapagent
