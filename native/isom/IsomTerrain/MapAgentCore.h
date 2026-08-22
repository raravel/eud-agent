#pragma once

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

namespace mapagent {

int mapEdit(
    const char* inputMapPath,
    const char* outputMapPath,
    const char* starCraftPath,
    const std::uint8_t* batchJson,
    std::size_t batchLength,
    std::string& reportJson);

int renderRegion(
    const char* mapPath,
    const char* starCraftPath,
    const std::uint8_t* requestJson,
    std::size_t requestLength,
    std::vector<std::uint8_t>& rgba,
    std::uint32_t& width,
    std::uint32_t& height);

int catalogQuery(
    const char* starCraftPath,
    const std::uint8_t* requestJson,
    std::size_t requestLength,
    std::string& resultJson);

int imageQuantize(
    const char* starCraftPath,
    std::uint16_t tileset,
    const std::uint8_t* rgba,
    std::size_t rgbaLength,
    std::uint16_t width,
    std::uint16_t height,
    const std::uint16_t* beforeTiles,
    std::size_t beforeTileCount,
    std::vector<std::uint8_t>& result);

int mapDigest(const char* mapPath, std::string& resultJson);

} // namespace mapagent
