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

int mapNew(
    const char* outputMapPath,
    const char* starCraftPath,
    const std::uint8_t* specJson,
    std::size_t specLength,
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

int gameAsset(
    const char* starCraftPath,
    const char* archivePath,
    std::vector<std::uint8_t>& result);

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

int mapAsset(
    const char* mapPath,
    const char* mpqPath,
    std::size_t maxBytes,
    std::vector<std::uint8_t>& result);

int mapSoundAdd(
    const char* inputMapPath,
    const char* outputMapPath,
    const char* expectedInputSha256,
    const char* destinationMpqPath,
    const std::uint8_t* oggBytes,
    std::size_t oggLength,
    std::string& reportJson);

int mapSoundAddBatch(
    const char* inputMapPath,
    const char* outputMapPath,
    const char* expectedInputSha256,
    const char* const* destinationMpqPaths,
    const std::uint8_t* const* oggBytes,
    const std::size_t* oggLengths,
    std::size_t count,
    std::string& reportJson);

int mapSoundReplace(
    const char* inputMapPath,
    const char* outputMapPath,
    const char* expectedInputSha256,
    const char* oldMpqPath,
    const char* destinationMpqPath,
    const std::uint8_t* oggBytes,
    std::size_t oggLength,
    std::string& reportJson);

int mapSoundRemove(
    const char* inputMapPath,
    const char* outputMapPath,
    const char* expectedInputSha256,
    const std::uint16_t* soundIndexes,
    std::size_t count,
    std::string& reportJson);

} // namespace mapagent
