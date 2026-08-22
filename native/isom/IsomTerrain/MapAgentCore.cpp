#include "MapAgentCore.h"
#include "MapAgentJson.h"
#include "IsomApi.h"

#pragma warning(push, 0)
#include "../MappingCoreLib/MappingCore.h"
#include "../MappingCoreLib/sha256.h"
#include "../StormLib/src/StormLib.h"
#pragma warning(pop)

#include <Windows.h>

#include <algorithm>
#include <chrono>
#include <array>
#include <cctype>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <limits>
#include <map>
#include <memory>
#include <mutex>
#include <set>
#include <optional>
#include <sstream>
#include <thread>
#include <tuple>

namespace mapagent {
namespace {

constexpr const char* EditSchema = "eud-map-edit/1";
constexpr const char* RenderSchema = "eud-map-render/1";
constexpr const char* CatalogSchema = "eud-map-catalog/1";
constexpr std::size_t MaxOperations = 4096;

[[noreturn]] void fail(const std::string& message)
{
    throw JsonError(message);
}

std::size_t checkedSize(const Json::Object& object, const char* field, const std::string& context, bool positive = false)
{
    return sizeValue(requiredField(object, field, context), context + "." + field, positive);
}

std::uint16_t checkedU16(const Json::Object& object, const char* field, const std::string& context)
{
    const std::size_t value = checkedSize(object, field, context);
    if ( value > std::numeric_limits<std::uint16_t>::max() )
        fail(context + "." + field + ": value exceeds uint16");
    return static_cast<std::uint16_t>(value);
}

std::uint32_t checkedU32(const Json::Object& object, const char* field, const std::string& context)
{
    const std::size_t value = checkedSize(object, field, context);
    if ( value > std::numeric_limits<std::uint32_t>::max() )
        fail(context + "." + field + ": value exceeds uint32");
    return static_cast<std::uint32_t>(value);
}

std::uint8_t optionalU8(const Json::Object& object, const char* field, std::uint8_t fallback, const std::string& context)
{
    const Json* value = optionalField(object, field);
    if ( value == nullptr )
        return fallback;
    const std::size_t number = sizeValue(*value, context + "." + field);
    if ( number > std::numeric_limits<std::uint8_t>::max() )
        fail(context + "." + field + ": value exceeds uint8");
    return static_cast<std::uint8_t>(number);
}

std::uint16_t optionalU16(const Json::Object& object, const char* field, std::uint16_t fallback, const std::string& context)
{
    const Json* value = optionalField(object, field);
    if ( value == nullptr )
        return fallback;
    const std::size_t number = sizeValue(*value, context + "." + field);
    if ( number > std::numeric_limits<std::uint16_t>::max() )
        fail(context + "." + field + ": value exceeds uint16");
    return static_cast<std::uint16_t>(number);
}

std::uint32_t optionalU32(const Json::Object& object, const char* field, std::uint32_t fallback, const std::string& context)
{
    const Json* value = optionalField(object, field);
    if ( value == nullptr )
        return fallback;
    const std::size_t number = sizeValue(*value, context + "." + field);
    if ( number > std::numeric_limits<std::uint32_t>::max() )
        fail(context + "." + field + ": value exceeds uint32");
    return static_cast<std::uint32_t>(number);
}

bool optionalBool(const Json::Object& object, const char* field, bool fallback, const std::string& context)
{
    const Json* value = optionalField(object, field);
    return value == nullptr ? fallback : boolValue(*value, context + "." + field);
}

std::vector<std::uint8_t> decodeHex(const std::string& text, const std::string& context)
{
    if ( text.size() % 2 != 0 )
        fail(context + ": hex string must contain an even number of digits");
    auto nibble = [&](char value) -> std::uint8_t {
        if ( value >= '0' && value <= '9' ) return static_cast<std::uint8_t>(value - '0');
        if ( value >= 'a' && value <= 'f' ) return static_cast<std::uint8_t>(value - 'a' + 10);
        if ( value >= 'A' && value <= 'F' ) return static_cast<std::uint8_t>(value - 'A' + 10);
        fail(context + ": invalid hex digit");
    };
    std::vector<std::uint8_t> bytes(text.size() / 2);
    for ( std::size_t index = 0; index < bytes.size(); ++index )
        bytes[index] = static_cast<std::uint8_t>((nibble(text[index * 2]) << 4) | nibble(text[index * 2 + 1]));
    return bytes;
}

std::string sha256Bytes(const void* data, std::size_t size)
{
    SHA256 sha;
    return sha(data, size);
}

std::string readFileSha256(const std::string& path)
{
    std::ifstream input(path, std::ios::binary);
    if ( !input )
        fail("cannot open input map: " + path);
    SHA256 sha;
    std::array<char, 64 * 1024> buffer{};
    while ( input )
    {
        input.read(buffer.data(), static_cast<std::streamsize>(buffer.size()));
        const std::streamsize count = input.gcount();
        if ( count > 0 )
            sha.add(buffer.data(), static_cast<std::size_t>(count));
    }
    if ( !input.eof() )
        fail("cannot read input map: " + path);
    return sha.getHash();
}

std::string lowerAscii(std::string value)
{
    std::transform(value.begin(), value.end(), value.begin(), [](unsigned char c) {
        return static_cast<char>(std::tolower(c));
    });
    return value;
}

Sc::Terrain::Tileset parseTileset(const std::string& name)
{
    const std::string value = lowerAscii(name);
    if ( value == "badlands" ) return Sc::Terrain::Tileset::Badlands;
    if ( value == "platform" || value == "spaceplatform" ) return Sc::Terrain::Tileset::SpacePlatform;
    if ( value == "installation" ) return Sc::Terrain::Tileset::Installation;
    if ( value == "ashworld" ) return Sc::Terrain::Tileset::Ashworld;
    if ( value == "jungle" ) return Sc::Terrain::Tileset::Jungle;
    if ( value == "desert" ) return Sc::Terrain::Tileset::Desert;
    if ( value == "arctic" || value == "ice" ) return Sc::Terrain::Tileset::Arctic;
    if ( value == "twilight" ) return Sc::Terrain::Tileset::Twilight;
    fail("unknown tileset: " + name);
}

std::string tilesetName(Sc::Terrain::Tileset tileset)
{
    static const std::array<const char*, 8> names{
        "badlands", "platform", "installation", "ashworld", "jungle", "desert", "arctic", "twilight"
    };
    return names[static_cast<std::size_t>(tileset) % names.size()];
}
std::vector<ArchiveFilePtr> openScDataFilesSilent(const std::string& starCraftPath)
{
    std::vector<Sc::DataFile::Descriptor> dataFiles;
    for ( const auto& descriptor : Sc::DataFile::getDefaultDataFiles() )
    {
        dataFiles.emplace_back(descriptor.getPriority(), descriptor.isCasc(), descriptor.isOptionalIfCascFound(),
            descriptor.getFileName(), descriptor.getExpectedFilePath(), nullptr, descriptor.isExpectedInScDirectory());
    }
    Sc::DataFile::Browser browser;
    for ( int attempt = 0; attempt < 3; ++attempt )
    {
        if ( attempt != 0 )
            std::this_thread::sleep_for(std::chrono::milliseconds(250));
        std::vector<ArchiveFilePtr> files = browser.openScDataFiles(dataFiles, starCraftPath, nullptr);
        if ( !files.empty() )
            return files;
    }
    return {};
}

struct StarImage {
    std::uint16_t x;
    std::uint16_t y;
    std::uint16_t width;
    std::uint16_t height;
    std::size_t pixelsOffset;
};

struct Assets {
    std::vector<ArchiveFilePtr> archives;
    Sc::Terrain terrain;
    Sc::Terrain_ isom;
    Sc::Unit units;
    Sc::Pcx tunit;
    Sc::TblFile images;
    Sc::TblFile statTxt;
    std::vector<std::uint8_t> starData;
    std::vector<std::vector<StarImage>> starLayers;
    std::vector<Sc::Sprite::DatEntry> spriteEntries;
    std::vector<Sc::Sprite::ImageDatEntry> imageEntries;
    std::array<std::vector<Sc::Terrain::Doodad>, Sc::Terrain::NumTilesets> doodadsByTileset;
    std::map<std::size_t, std::unique_ptr<Sc::Sprite::Grp>> grpCache;
    std::mutex grpMutex;

    bool loadStarMetadata()
    {
        auto data = Sc::Data::GetAsset(archives, "SD\\TileSet\\platform.spk");
        if ( !data.has_value() )
            data = Sc::Data::GetAsset(archives, "parallax\\star.spk");
        if ( !data.has_value() || data->size() < sizeof(std::uint16_t) )
            return false;

        const auto readU16 = [&data](std::size_t offset) {
            return static_cast<std::uint16_t>(
                (*data)[offset] | static_cast<std::uint16_t>((*data)[offset + 1]) << 8);
        };
        const auto readU32 = [&data](std::size_t offset) {
            return static_cast<std::uint32_t>(
                (*data)[offset] |
                static_cast<std::uint32_t>((*data)[offset + 1]) << 8 |
                static_cast<std::uint32_t>((*data)[offset + 2]) << 16 |
                static_cast<std::uint32_t>((*data)[offset + 3]) << 24);
        };

        const std::size_t layerCount = readU16(0);
        if ( layerCount == 0 || layerCount > 64 ||
             layerCount > (data->size() - sizeof(std::uint16_t)) / sizeof(std::uint16_t) )
            return false;

        const std::size_t positionsOffset =
            sizeof(std::uint16_t) + layerCount * sizeof(std::uint16_t);
        std::size_t starCount = 0;
        for ( std::size_t layer = 0; layer < layerCount; ++layer )
        {
            const std::size_t count = readU16(sizeof(std::uint16_t) + layer * sizeof(std::uint16_t));
            if ( count > (data->size() - positionsOffset) / 8 - starCount )
                return false;
            starCount += count;
        }

        std::vector<std::vector<StarImage>> layers;
        layers.reserve(layerCount);
        std::size_t visited = 0;
        for ( std::size_t layer = 0; layer < layerCount; ++layer )
        {
            const std::size_t count = readU16(sizeof(std::uint16_t) + layer * sizeof(std::uint16_t));
            auto& stars = layers.emplace_back();
            stars.reserve(count);
            for ( std::size_t index = 0; index < count; ++index )
            {
                const std::size_t position = positionsOffset + (visited + index) * 8;
                const std::size_t bitmap = readU32(position + 4);
                if ( bitmap > data->size() || data->size() - bitmap < 4 )
                    return false;
                const std::size_t width = readU16(bitmap);
                const std::size_t height = readU16(bitmap + 2);
                if ( width == 0 || height == 0 ||
                     width > (data->size() - bitmap - 4) / height )
                    return false;
                stars.push_back(StarImage{
                    readU16(position),
                    readU16(position + 2),
                    static_cast<std::uint16_t>(width),
                    static_cast<std::uint16_t>(height),
                    bitmap + 4
                });
            }
            visited += count;
        }
        starData = std::move(*data);
        starLayers = std::move(layers);
        return true;
    }

    bool loadSpriteMetadata()
    {
        const auto imageData = Sc::Data::GetAsset(archives, "arr\\images.dat");
        if ( !imageData.has_value() || imageData->size() != sizeof(Sc::Sprite::ImageDatFile) )
            return false;
        const auto& imagesDat = reinterpret_cast<const Sc::Sprite::ImageDatFile&>((*imageData)[0]);
        imageEntries.reserve(Sc::Sprite::TotalImages);
        for ( std::size_t index = 0; index < Sc::Sprite::TotalImages; ++index )
        {
            imageEntries.push_back(Sc::Sprite::ImageDatEntry{
                imagesDat.grpFile[index], imagesDat.graphicTurns[index], imagesDat.clickable[index],
                imagesDat.useFullIscript[index], imagesDat.drawIfCloaked[index], imagesDat.drawFunction[index],
                imagesDat.remapping[index], imagesDat.iScriptId[index], imagesDat.shieldOverlay[index],
                imagesDat.attackOverlay[index], imagesDat.damageOverlay[index], imagesDat.specialOverlay[index],
                imagesDat.landingDustOverlay[index], imagesDat.liftOffOverlay[index]
            });
        }

        const auto spriteData = Sc::Data::GetAsset(archives, "arr\\sprites.dat");
        if ( !spriteData.has_value() || spriteData->size() != sizeof(Sc::Sprite::DatFile) )
            return false;
        const auto& spritesDat = reinterpret_cast<const Sc::Sprite::DatFile&>((*spriteData)[0]);
        spriteEntries.reserve(Sc::Sprite::TotalSprites);
        std::size_t index = 0;
        for ( ; index < Sc::Sprite::DatFile::IdRange::From0To129; ++index )
            spriteEntries.push_back(Sc::Sprite::DatEntry{spritesDat.imageFile[index], 0, spritesDat.unknown[index], spritesDat.isVisible[index], 0, 0});
        for ( ; index < Sc::Sprite::TotalSprites; ++index )
        {
            spriteEntries.push_back(Sc::Sprite::DatEntry{
                spritesDat.imageFile[index],
                spritesDat.healthBar[index - Sc::Sprite::DatFile::IdRange::From0To129],
                spritesDat.unknown[index],
                spritesDat.isVisible[index],
                spritesDat.selectionCircleImage[index - Sc::Sprite::DatFile::IdRange::From0To129],
                spritesDat.selectionCircleOffset[index - Sc::Sprite::DatFile::IdRange::From0To129]
            });
        }
        return true;
    }

    bool loadDoodadMetadata()
    {
        const std::size_t offset = Sc::Terrain::Cv5Dat::MaxTileGroups * sizeof(Sc::Terrain::TileGroup);
        for ( std::size_t tileset = 0; tileset < Sc::Terrain::NumTilesets; ++tileset )
        {
            const std::string path = "tileset\\" + Sc::Terrain::TilesetNames[tileset] + ".cv5";
            const auto data = Sc::Data::GetAsset(archives, path);
            if ( !data.has_value() || data->size() < offset || (data->size() - offset) % sizeof(Sc::Terrain::Doodad) != 0 )
                return false;
            const std::size_t count = (data->size() - offset) / sizeof(Sc::Terrain::Doodad);
            const auto* records = reinterpret_cast<const Sc::Terrain::Doodad*>(data->data() + offset);
            doodadsByTileset[tileset].assign(records, records + count);
        }
        return true;
    }

    const std::vector<Sc::Terrain::Doodad>& doodads(Sc::Terrain::Tileset tileset) const
    {
        return doodadsByTileset[static_cast<std::size_t>(tileset) % doodadsByTileset.size()];
    }

    const Sc::Sprite::DatEntry& sprite(std::size_t index) const
    {
        if ( index >= spriteEntries.size() ) fail("sprite index is out of range");
        return spriteEntries[index];
    }

    const Sc::Sprite::ImageDatEntry& image(std::size_t index) const
    {
        if ( index >= imageEntries.size() ) fail("image index is out of range");
        return imageEntries[index];
    }

    const Sc::Sprite::Grp& grp(std::size_t index)
    {
        std::lock_guard<std::mutex> guard(grpMutex);
        const auto found = grpCache.find(index);
        if ( found != grpCache.end() )
            return *found->second;
        auto value = std::make_unique<Sc::Sprite::Grp>();
        if ( index == 0 )
            value->makeBlank();
        else
        {
            std::string path;
            if ( !images.getString(index, path) || path.empty() || !value->load(archives, "unit\\" + path) )
                fail("cannot load GRP index " + std::to_string(index));
        }
        const auto inserted = grpCache.emplace(index, std::move(value));
        return *inserted.first->second;
    }
};

std::shared_ptr<Assets> loadAssets(const std::string& root)
{
    static std::mutex cacheMutex;
    static std::map<std::string, std::shared_ptr<Assets>> cache;
    const std::string key = lowerAscii(root);
    std::lock_guard<std::mutex> guard(cacheMutex);
    const auto found = cache.find(key);
    if ( found != cache.end() )
        return found->second;

    auto assets = std::make_shared<Assets>();
    assets->archives = openScDataFilesSilent(root);
    if ( assets->archives.empty() )
        fail("cannot open StarCraft data at: " + root);
    assets->loadStarMetadata();
    if ( !assets->terrain.load(assets->archives) ) fail("cannot load StarCraft terrain assets");
    if ( !assets->loadDoodadMetadata() ) fail("cannot load StarCraft doodad metadata");
    if ( !assets->isom.load(assets->archives) ) fail("cannot load StarCraft ISOM catalog");
    if ( !assets->units.load(assets->archives) ) fail("cannot load units.dat/flingy.dat");
    if ( !assets->loadSpriteMetadata() ) fail("cannot load sprite/image DAT metadata");
    if ( !assets->tunit.load(assets->archives, "game\\tunit.pcx") ) fail("cannot load player color palette");
    if ( !assets->images.load(assets->archives, "arr\\images.tbl") ) fail("cannot load images.tbl");
    if ( !assets->statTxt.load(assets->archives, "rez\\stat_txt.tbl") ) fail("cannot load stat_txt.tbl");
    cache.emplace(key, assets);
    return assets;
}


bool tileGraphicsValid(const Sc::Terrain::Tiles& tiles, std::uint16_t tile)
{
    const std::size_t group = static_cast<std::size_t>(tile) / 16;
    if ( group >= tiles.tileGroups.size() )
        return false;
    const std::size_t mega = tiles.tileGroups[group].megaTileIndex[tile % 16];
    if ( mega >= tiles.tileGraphics.size() || mega >= tiles.tileFlags.size() )
        return false;
    const auto& graphics = tiles.tileGraphics[mega];
    for ( std::size_t y = 0; y < 4; ++y )
    {
        for ( std::size_t x = 0; x < 4; ++x )
        {
            if ( graphics.miniTileGraphics[y][x].vr4Index() >= tiles.miniTilePixels.size() )
                return false;
        }
    }
    return true;
}

ScMap copyToScMap(const MapFile& source)
{
    ScMap destination;
    destination.tileWidth = static_cast<std::uint16_t>(source.getTileWidth());
    destination.tileHeight = static_cast<std::uint16_t>(source.getTileHeight());
    destination.tileset = source.getTileset();
    destination.isomRects.assign(source.isomRects.size(), {});
    if ( !source.isomRects.empty() )
        std::memcpy(destination.isomRects.data(), source.isomRects.data(), source.isomRects.size() * sizeof(Chk::IsomRect));
    destination.editorTiles = source.editorTiles;
    destination.tiles = source.tiles;
    return destination;
}

void copyFromScMap(MapFile& destination, const ScMap& source)
{
    destination.dimensions.tileWidth = source.tileWidth;
    destination.dimensions.tileHeight = source.tileHeight;
    destination.tileset = source.tileset;
    destination.isomRects.assign(source.isomRects.size(), {});
    if ( !source.isomRects.empty() )
        std::memcpy(destination.isomRects.data(), source.isomRects.data(), source.isomRects.size() * sizeof(Chk::IsomRect));
    destination.editorTiles = source.editorTiles;
    destination.tiles = source.tiles;
}

void setExactTile(MapFile& map, std::size_t x, std::size_t y, std::uint16_t tile)
{
    if ( x >= map.getTileWidth() || y >= map.getTileHeight() )
        fail("terrain coordinate is outside map bounds");
    const std::size_t expected = map.getTileWidth() * map.getTileHeight();
    if ( map.tiles.size() != expected )
        fail("MTXM size does not match DIM");
    if ( map.editorTiles.size() != expected )
        map.editorTiles = map.tiles;
    const std::size_t index = y * map.getTileWidth() + x;
    map.tiles[index] = tile;
    map.editorTiles[index] = tile;
}

std::string unitFingerprint(const Chk::Unit& unit)
{
    return sha256Bytes(&unit, sizeof(unit));
}

std::string doodadFingerprint(const Chk::Doodad& doodad)
{
    return sha256Bytes(&doodad, sizeof(doodad));
}

std::string spriteFingerprint(const Chk::Sprite& sprite)
{
    return sha256Bytes(&sprite, sizeof(sprite));
}

Chk::Unit parseUnitState(const Json& value, const std::string& context)
{
    const auto& object = objectValue(value, context);
    allowedFields(object, {"typeId", "owner", "x", "y", "classId", "relationFlags", "validStateFlags",
        "validFieldFlags", "hpPercent", "shieldPercent", "energyPercent", "resourceAmount", "hangarAmount",
        "stateFlags", "unused", "relationClassId"}, context);
    const std::uint16_t typeId = checkedU16(object, "typeId", context);
    if ( typeId >= Sc::Unit::TotalTypes ) fail(context + ": unit type is out of range");
    Chk::Unit unit{};
    unit.type = Sc::Unit::Type(typeId);
    unit.owner = optionalU8(object, "owner", 0, context);
    unit.xc = checkedU16(object, "x", context);
    unit.yc = checkedU16(object, "y", context);
    unit.classId = optionalU32(object, "classId", 0, context);
    unit.relationFlags = optionalU16(object, "relationFlags", 0, context);
    unit.validStateFlags = optionalU16(object, "validStateFlags", 0, context);
    unit.validFieldFlags = optionalU16(object, "validFieldFlags", 0, context);
    unit.hitpointPercent = optionalU8(object, "hpPercent", 100, context);
    unit.shieldPercent = optionalU8(object, "shieldPercent", 100, context);
    unit.energyPercent = optionalU8(object, "energyPercent", 100, context);
    unit.resourceAmount = optionalU32(object, "resourceAmount", 0, context);
    unit.hangerAmount = optionalU16(object, "hangarAmount", 0, context);
    unit.stateFlags = optionalU16(object, "stateFlags", 0, context);
    unit.unused = optionalU32(object, "unused", 0, context);
    unit.relationClassId = optionalU32(object, "relationClassId", 0, context);
    if ( unit.owner > 11 ) fail(context + ": owner must be 0..11");
    if ( unit.hitpointPercent > 100 || unit.shieldPercent > 100 || unit.energyPercent > 100 )
        fail(context + ": hp/shield/energy percentages must be 0..100");
    return unit;
}

Chk::Doodad parseDoodadState(const Json& value, const std::string& context)
{
    const auto& object = objectValue(value, context);
    allowedFields(object, {"doodadId", "x", "y", "owner", "disabled"}, context);
    Chk::Doodad doodad{};
    doodad.type = Sc::Terrain::Doodad::Type(checkedU16(object, "doodadId", context));
    doodad.xc = checkedU16(object, "x", context);
    doodad.yc = checkedU16(object, "y", context);
    doodad.owner = optionalU8(object, "owner", 11, context);
    doodad.enabled = optionalBool(object, "disabled", false, context)
        ? Chk::Doodad::Enabled::Disabled : Chk::Doodad::Enabled::Enabled;
    if ( doodad.owner > 11 ) fail(context + ": owner must be 0..11");
    return doodad;
}

Chk::Sprite parseSpriteState(const Json& value, const std::string& context)
{
    const auto& object = objectValue(value, context);
    allowedFields(object, {"spriteId", "x", "y", "owner", "flags"}, context);
    Chk::Sprite sprite{};
    sprite.type = Sc::Sprite::Type(checkedU16(object, "spriteId", context));
    sprite.xc = checkedU16(object, "x", context);
    sprite.yc = checkedU16(object, "y", context);
    sprite.owner = optionalU8(object, "owner", 11, context);
    sprite.unused = 0;
    sprite.flags = optionalU16(object, "flags", static_cast<std::uint16_t>(Chk::Sprite::SpriteFlags::DrawAsSprite), context);
    if ( static_cast<std::size_t>(sprite.type) >= Sc::Sprite::TotalSprites ) fail(context + ": sprite type is out of range");
    if ( sprite.owner > 11 ) fail(context + ": owner must be 0..11");
    return sprite;
}

void requirePosition(std::uint16_t x, std::uint16_t y, const MapFile& map, const std::string& context)
{
    if ( x >= map.getPixelWidth() || y >= map.getPixelHeight() )
        fail(context + ": object position is outside map bounds");
}

bool blankLocation(const Chk::Location& location)
{
    return location.left == 0 && location.top == 0 && location.right == 0 && location.bottom == 0 && location.stringId == 0;
}

void requireLocationId(const MapFile& map, std::size_t id, const std::string& context)
{
    if ( id < 1 || id > map.numLocations() ) fail(context + ": location id is out of range");
    if ( id == static_cast<std::size_t>(Chk::LocationId::Anywhere) ) fail(context + ": location #64 Anywhere is protected");
}

std::string rawStringFromHex(const std::string& text, const std::string& context)
{
    const auto bytes = decodeHex(text, context);
    if ( std::find(bytes.begin(), bytes.end(), 0) != bytes.end() )
        fail(context + ": location names cannot contain NUL");
    return std::string(reinterpret_cast<const char*>(bytes.data()), bytes.size());
}

void applyDoodadFootprint(MapFile& map, const Chk::Doodad& doodad, const Assets& assets, const std::string& context)
{
    const auto& tiles = assets.terrain.get(map.getTileset());
    const auto& doodads = assets.doodads(map.getTileset());
    const std::size_t id = static_cast<std::size_t>(doodad.type);
    if ( id >= doodads.size() ) fail(context + ": doodad id is not valid for this tileset");
    const auto& record = doodads[id];
    if ( record.doodadWidth == 0 || record.doodadHeight == 0 || record.doodadWidth * record.doodadHeight > 16 )
        fail(context + ": doodad footprint is invalid");
    const int centerX = static_cast<int>(doodad.xc / 32);
    const int centerY = static_cast<int>(doodad.yc / 32);
    const int left = centerX - static_cast<int>(record.doodadWidth / 2);
    const int top = centerY - static_cast<int>(record.doodadHeight / 2);
    if ( left < 0 || top < 0 || left + record.doodadWidth > static_cast<int>(map.getTileWidth()) ||
         top + record.doodadHeight > static_cast<int>(map.getTileHeight()) )
        fail(context + ": doodad footprint is outside map bounds");
    for ( std::size_t y = 0; y < record.doodadHeight; ++y )
    {
        for ( std::size_t x = 0; x < record.doodadWidth; ++x )
        {
            const std::uint16_t tile = static_cast<std::uint16_t>((Sc::Terrain::Cv5Dat::MaxTileGroups + id) * 16 + y * record.doodadWidth + x);
            if ( !tileGraphicsValid(tiles, tile) ) fail(context + ": doodad references invalid graphics");
            setExactTile(map, static_cast<std::size_t>(left) + x, static_cast<std::size_t>(top) + y, tile);
        }
    }
}

void replaceDoodadFootprint(MapFile& map, const Chk::Doodad& doodad, const Json& replacement,
    const Assets& assets, const std::string& context)
{
    const auto& tiles = assets.terrain.get(map.getTileset());
    const auto& doodads = assets.doodads(map.getTileset());
    const std::size_t id = static_cast<std::size_t>(doodad.type);
    if ( id >= doodads.size() ) fail(context + ": doodad id is not valid for this tileset");
    const auto& record = doodads[id];
    const auto& rows = arrayValue(replacement, context + ".replacementTiles");
    if ( rows.size() != record.doodadHeight )
        fail(context + ": replacementTiles height must match the old doodad footprint");
    const int centerX = static_cast<int>(doodad.xc / 32);
    const int centerY = static_cast<int>(doodad.yc / 32);
    const int left = centerX - static_cast<int>(record.doodadWidth / 2);
    const int top = centerY - static_cast<int>(record.doodadHeight / 2);
    if ( left < 0 || top < 0 || left + record.doodadWidth > static_cast<int>(map.getTileWidth()) ||
         top + record.doodadHeight > static_cast<int>(map.getTileHeight()) )
        fail(context + ": old doodad footprint is outside map bounds");
    for ( std::size_t y = 0; y < rows.size(); ++y )
    {
        const auto& row = arrayValue(rows[y], context + ".replacementTiles[]");
        if ( row.size() != record.doodadWidth )
            fail(context + ": replacementTiles width must match the old doodad footprint");
        for ( std::size_t x = 0; x < row.size(); ++x )
        {
            const std::size_t value = sizeValue(row[x], context + ".replacementTiles[][]");
            if ( value > std::numeric_limits<std::uint16_t>::max() ||
                 !tileGraphicsValid(tiles, static_cast<std::uint16_t>(value)) )
                fail(context + ": replacementTiles contains invalid tile graphics");
            setExactTile(map, static_cast<std::size_t>(left) + x,
                static_cast<std::size_t>(top) + y, static_cast<std::uint16_t>(value));
        }
    }
}
std::optional<Chk::Sprite> doodadOverlay(const MapFile& map, const Chk::Doodad& doodad, const Assets& assets, const std::string& context)
{

    const auto& doodads = assets.doodads(map.getTileset());
    const std::size_t id = static_cast<std::size_t>(doodad.type);
    if ( id >= doodads.size() ) fail(context + ": doodad id is not valid for this tileset");
    const auto& record = doodads[id];
    if ( (record.flags & 0x30) == 0 )
        return std::nullopt;
    Chk::Sprite sprite{};
    sprite.type = Sc::Sprite::Type(record.overlayIndex);
    sprite.xc = doodad.xc;
    sprite.yc = doodad.yc;
    sprite.owner = doodad.owner;
    sprite.unused = 0;
    sprite.flags = (record.flags & 0x10) != 0 ? static_cast<std::uint16_t>(Chk::Sprite::SpriteFlags::DrawAsSprite) : 0;
    if ( static_cast<std::size_t>(sprite.type) >= Sc::Sprite::TotalSprites )
        fail(context + ": doodad overlay sprite is out of range");
    return sprite;
}

void addDoodadOverlay(MapFile& map, const Chk::Doodad& doodad, const Assets& assets, const std::string& context)
{
    const auto overlay = doodadOverlay(map, doodad, assets, context);
    if ( overlay.has_value() )
        map.addSprite(*overlay);
}

void deleteDoodadOverlay(MapFile& map, const Chk::Doodad& doodad, const Assets& assets, const std::string& context)
{
    const auto overlay = doodadOverlay(map, doodad, assets, context);
    if ( !overlay.has_value() )
        return;
    for ( std::size_t index = map.numSprites(); index-- > 0; )
    {
        const auto& candidate = map.getSprite(index);
        if ( candidate.type == overlay->type && candidate.xc == overlay->xc && candidate.yc == overlay->yc &&
             candidate.owner == overlay->owner && candidate.flags == overlay->flags )
        {
            map.deleteSprite(index);
            return;
        }
    }
}

Json::Object effect(const std::string& operation, const std::string& layer, std::size_t ordinal)
{
    return Json::Object{{"op", operation}, {"layer", layer}, {"ordinal", ordinal}};
}

std::string temporaryOutputPath(const std::string& output)
{
    return output + ".native-" + std::to_string(GetCurrentProcessId()) + "-" + std::to_string(GetTickCount64()) + ".tmp";
}

bool replaceFile(const std::string& source, const std::string& destination)
{
    return ::MoveFileExA(source.c_str(), destination.c_str(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) != FALSE;
}

using AssetInventory = std::map<std::string, std::string>;

bool ignoredMpqAsset(const std::string& name)
{
    const std::string normalized = lowerAscii(name);
    return normalized == "staredit\\scenario.chk" || normalized == "scenario.chk" ||
        normalized == "(listfile)" || normalized == "(attributes)" || normalized == "(signature)";
}

std::wstring utf8Wide(const std::string& text)
{
    const int size = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, text.c_str(), -1, nullptr, 0);
    if ( size <= 0 ) fail("map path is not valid UTF-8");
    std::wstring wide(static_cast<std::size_t>(size), L'\0');
    if ( MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, text.c_str(), -1, wide.data(), size) != size )
        fail("map path conversion failed");
    return wide;
}

AssetInventory inventoryMpq(const std::string& path)
{
    HANDLE archive = nullptr;
    const std::wstring widePath = utf8Wide(path);
    if ( !SFileOpenArchive(widePath.c_str(), 0, STREAM_FLAG_READ_ONLY, &archive) )
        fail("cannot open MPQ inventory: " + path);
    AssetInventory inventory;
    SFILE_FIND_DATA found{};
    HANDLE search = SFileFindFirstFile(archive, "*", &found, nullptr);
    if ( search != nullptr )
    {
        do
        {
            const std::string name(found.cFileName);
            if ( ignoredMpqAsset(name) )
                continue;
            HANDLE file = nullptr;
            if ( !SFileOpenFileEx(archive, name.c_str(), SFILE_OPEN_FROM_MPQ, &file) )
            {
                SFileFindClose(search);
                SFileCloseArchive(archive);
                fail("cannot open MPQ asset: " + name);
            }
            DWORD high = 0;
            const DWORD low = SFileGetFileSize(file, &high);
            if ( low == SFILE_INVALID_SIZE || high != 0 )
            {
                SFileCloseFile(file);
                SFileFindClose(search);
                SFileCloseArchive(archive);
                fail("MPQ asset is too large: " + name);
            }
            std::vector<std::uint8_t> bytes(low);
            DWORD read = 0;
            if ( low != 0 && (!SFileReadFile(file, bytes.data(), low, &read, nullptr) || read != low) )
            {
                SFileCloseFile(file);
                SFileFindClose(search);
                SFileCloseArchive(archive);
                fail("cannot read MPQ asset: " + name);
            }
            SFileCloseFile(file);
            inventory[lowerAscii(name)] = sha256Bytes(bytes.data(), bytes.size());
        }
        while ( SFileFindNextFile(search, &found) );
        SFileFindClose(search);
    }
    SFileCloseArchive(archive);
    return inventory;
}

std::string inventoryDigest(const AssetInventory& inventory)
{
    SHA256 sha;
    for ( const auto& entry : inventory )
    {
        sha.add(entry.first.data(), entry.first.size());
        sha.add("\0", 1);
        sha.add(entry.second.data(), entry.second.size());
        sha.add("\0", 1);
    }
    return sha.getHash();
}

Json inventoryJson(const AssetInventory& inventory)
{
    Json::Array assets;
    for ( const auto& entry : inventory )
        assets.emplace_back(Json::Object{{"path", entry.first}, {"sha256", entry.second}});
    return Json::Object{{"digest", inventoryDigest(inventory)}, {"assets", std::move(assets)}};
}

void drawStarParallax(std::vector<std::uint8_t>& rgba, std::size_t outWidth, std::size_t outHeight,
    std::int64_t cropLeft, std::int64_t cropTop, std::size_t scale,
    const Assets& assets, const Sc::Terrain::Tiles& tiles)
{
    constexpr std::int64_t RepeatWidth = 648;
    constexpr std::int64_t RepeatHeight = 488;
    const std::int64_t cropRight = cropLeft + static_cast<std::int64_t>(outWidth * scale);
    const std::int64_t cropBottom = cropTop + static_cast<std::int64_t>(outHeight * scale);

    for ( auto layer = assets.starLayers.rbegin(); layer != assets.starLayers.rend(); ++layer )
    {
        for ( const auto& star : *layer )
        {
            const std::int64_t localLeft = static_cast<std::int64_t>(star.x) - 8;
            const std::int64_t localTop = static_cast<std::int64_t>(star.y) - 8;
            const std::int64_t repeatsBackX =
                (static_cast<std::int64_t>(star.width) + 15 + RepeatWidth - 1) / RepeatWidth;
            const std::int64_t repeatsBackY =
                (static_cast<std::int64_t>(star.height) + 15 + RepeatHeight - 1) / RepeatHeight;
            const std::int64_t firstOriginX =
                cropLeft / RepeatWidth * RepeatWidth - repeatsBackX * RepeatWidth;
            const std::int64_t firstOriginY =
                cropTop / RepeatHeight * RepeatHeight - repeatsBackY * RepeatHeight;

            for ( std::int64_t originY = firstOriginY;
                  originY + localTop < cropBottom; originY += RepeatHeight )
            {
                const std::int64_t starTop = originY + localTop;
                const std::int64_t starBottom = starTop + star.height;
                if ( starBottom <= cropTop )
                    continue;
                const std::size_t outTop = starTop <= cropTop ? 0 :
                    static_cast<std::size_t>((starTop - cropTop + scale - 1) / scale);
                const std::size_t outBottom = std::min(
                    outHeight,
                    static_cast<std::size_t>((starBottom - 1 - cropTop) / scale + 1));

                for ( std::int64_t originX = firstOriginX;
                      originX + localLeft < cropRight; originX += RepeatWidth )
                {
                    const std::int64_t starLeft = originX + localLeft;
                    const std::int64_t starRight = starLeft + star.width;
                    if ( starRight <= cropLeft )
                        continue;
                    const std::size_t outLeft = starLeft <= cropLeft ? 0 :
                        static_cast<std::size_t>((starLeft - cropLeft + scale - 1) / scale);
                    const std::size_t outRight = std::min(
                        outWidth,
                        static_cast<std::size_t>((starRight - 1 - cropLeft) / scale + 1));

                    for ( std::size_t outY = outTop; outY < outBottom; ++outY )
                    {
                        const std::size_t sourceY = static_cast<std::size_t>(
                            cropTop + static_cast<std::int64_t>(outY * scale) - starTop);
                        for ( std::size_t outX = outLeft; outX < outRight; ++outX )
                        {
                            const std::size_t sourceX = static_cast<std::size_t>(
                                cropLeft + static_cast<std::int64_t>(outX * scale) - starLeft);
                            const std::uint8_t palette = assets.starData[
                                star.pixelsOffset + sourceY * star.width + sourceX];
                            if ( palette == 0 )
                                continue;
                            const auto& color = tiles.systemColorPalette[palette];
                            const std::size_t output = (outY * outWidth + outX) * 4;
                            rgba[output] = color.red;
                            rgba[output + 1] = color.green;
                            rgba[output + 2] = color.blue;
                            rgba[output + 3] = 255;
                        }
                    }
                }
            }
        }
    }
}

void putTerrainPixel(std::vector<std::uint8_t>& rgba, std::size_t outWidth, std::size_t outX, std::size_t outY,
    std::uint16_t tile, std::size_t tilePixelX, std::size_t tilePixelY, const Sc::Terrain::Tiles& tiles)
{
    const std::size_t group = static_cast<std::size_t>(tile) / 16;
    if ( group >= tiles.tileGroups.size() )
        return;
    const std::size_t mega = tiles.tileGroups[group].megaTileIndex[tile % 16];
    if ( mega >= tiles.tileGraphics.size() )
        return;
    const auto& mini = tiles.tileGraphics[mega].miniTileGraphics[tilePixelY / 8][tilePixelX / 8];
    const std::size_t vr4 = mini.vr4Index();
    if ( vr4 >= tiles.miniTilePixels.size() )
        return;
    std::size_t subX = tilePixelX % 8;
    if ( mini.isFlipped() ) subX = 7 - subX;
    const std::uint8_t palette = tiles.miniTilePixels[vr4].wpeIndex[tilePixelY % 8][subX];
    if ( palette == 0 )
        return;
    const auto& color = tiles.systemColorPalette[palette];
    const std::size_t offset = (outY * outWidth + outX) * 4;
    rgba[offset] = color.red;
    rgba[offset + 1] = color.green;
    rgba[offset + 2] = color.blue;
    rgba[offset + 3] = 255;
}

Sc::SystemColor playerColor(const Assets& assets, const Sc::Terrain::Tiles& tiles, std::uint8_t palette, std::uint8_t owner)
{
    if ( palette >= 8 && palette < 16 )
    {
        const std::size_t index = static_cast<std::size_t>(owner % 12) * 8 + (palette - 8);
        if ( index < assets.tunit.palette.size() )
            return assets.tunit.palette[index];
    }
    return tiles.systemColorPalette[palette];
}

void drawGrp(std::vector<std::uint8_t>& rgba, std::size_t outWidth, std::size_t outHeight,
    std::int64_t cropLeft, std::int64_t cropTop, std::size_t scale, std::uint16_t spriteId,
    std::uint8_t owner, std::uint8_t direction, std::int64_t centerX, std::int64_t centerY,
    Assets& assets, const Sc::Terrain::Tiles& tiles)
{
    if ( spriteId >= assets.spriteEntries.size() ) return;
    const auto& sprite = assets.sprite(spriteId);
    if ( sprite.imageFile >= assets.imageEntries.size() ) return;
    const auto& image = assets.image(sprite.imageFile);
    if ( image.grpFile > assets.images.numStrings() ) return;
    const auto& grp = assets.grp(image.grpFile).get();
    if ( grp.numFrames == 0 ) return;
    std::size_t frameIndex = image.graphicTurns == 0 ? 0 : (static_cast<std::size_t>(direction) * image.graphicTurns / 256);
    if ( frameIndex >= grp.numFrames ) frameIndex = 0;
    const auto& header = grp.frameHeaders[frameIndex];
    if ( header.frameWidth == 0 || header.frameHeight == 0 ) return;
    const auto* base = reinterpret_cast<const std::uint8_t*>(&grp);
    const auto* frame = reinterpret_cast<const Sc::Sprite::GrpFrame*>(base + header.frameOffset);
    const std::int64_t originX = centerX - static_cast<std::int64_t>(grp.grpWidth / 2) + header.xOffset;
    const std::int64_t originY = centerY - static_cast<std::int64_t>(grp.grpHeight / 2) + header.yOffset;
    for ( std::size_t row = 0; row < header.frameHeight; ++row )
    {
        const auto* line = reinterpret_cast<const Sc::Sprite::PixelLine*>(
            base + header.frameOffset + frame->rowOffsets[row]);
        std::size_t column = 0;
        while ( column < header.frameWidth )
        {
            const std::size_t length = line->lineLength();
            if ( length == 0 ) break;
            if ( line->isTransparentLine() )
                column += length;
            else
            {
                for ( std::size_t pixel = 0; pixel < length && column < header.frameWidth; ++pixel, ++column )
                {
                    const std::uint8_t palette = line->isSolidLine() ? line->paletteIndex[0] : line->paletteIndex[pixel];
                    const std::int64_t mapX = originX + static_cast<std::int64_t>(column);
                    const std::int64_t mapY = originY + static_cast<std::int64_t>(row);
                    if ( mapX < cropLeft || mapY < cropTop ) continue;
                    const std::int64_t relativeX = mapX - cropLeft;
                    const std::int64_t relativeY = mapY - cropTop;
                    if ( relativeX < 0 || relativeY < 0 ) continue;
                    const std::size_t outX = static_cast<std::size_t>(relativeX) / scale;
                    const std::size_t outY = static_cast<std::size_t>(relativeY) / scale;
                    if ( outX >= outWidth || outY >= outHeight ) continue;
                    const auto color = playerColor(assets, tiles, palette, owner);
                    const std::size_t offset = (outY * outWidth + outX) * 4;
                    rgba[offset] = color.red;
                    rgba[offset + 1] = color.green;
                    rgba[offset + 2] = color.blue;
                    rgba[offset + 3] = 255;
                }
            }
            line = reinterpret_cast<const Sc::Sprite::PixelLine*>(reinterpret_cast<const std::uint8_t*>(line) + line->sizeInBytes());
        }
    }
}

std::uint16_t unitSpriteId(const Assets& assets, std::uint16_t unitType)
{
    if ( unitType >= Sc::Unit::TotalTypes ) return std::numeric_limits<std::uint16_t>::max();
    const auto& unit = assets.units.getUnit(Sc::Unit::Type(unitType));
    if ( unit.graphics >= Sc::Unit::TotalFlingies ) return std::numeric_limits<std::uint16_t>::max();
    return assets.units.getFlingy(unit.graphics).sprite;
}

std::string spriteName(const Assets& assets, std::size_t spriteId)
{
    if ( spriteId >= assets.spriteEntries.size() ) return "Sprite " + std::to_string(spriteId);
    const auto& sprite = assets.sprite(spriteId);
    if ( sprite.imageFile >= assets.imageEntries.size() ) return "Sprite " + std::to_string(spriteId);
    const auto& image = assets.image(sprite.imageFile);
    std::string path;
    if ( image.grpFile != 0 && assets.images.getString(image.grpFile, path) && !path.empty() )
        return "Sprite " + std::to_string(spriteId) + " · " + path;
    return "Sprite " + std::to_string(spriteId);
}

Json tileEntry(const Sc::Terrain::Tiles& tiles, Sc::Terrain::Tileset tileset, std::uint16_t tile)
{
    const std::size_t group = tile / 16;
    const std::size_t variant = tile % 16;
    Json::Object result{{"id", static_cast<std::size_t>(tile)}, {"name", "Tile " + std::to_string(tile)},
        {"tileset", tilesetName(tileset)}, {"group", group}, {"variant", variant}};
    if ( group >= tiles.tileGroups.size() )
    {
        result.emplace("graphicsValid", false);
        return result;
    }
    const auto& record = tiles.tileGroups[group];
    result.emplace("terrainType", static_cast<std::size_t>(record.terrainType));
    result.emplace("buildability", static_cast<std::size_t>(record.buildability));
    result.emplace("groundHeight", static_cast<std::size_t>(record.groundHeight));
    const std::size_t mega = record.megaTileIndex[variant];
    result.emplace("megaTile", mega);
    result.emplace("graphicsValid", tileGraphicsValid(tiles, tile));
    if ( mega < tiles.tileFlags.size() )
    {
        bool anyWalkable = false;
        bool allWalkable = true;
        bool ramp = false;
        bool blocksView = false;
        std::size_t high = 0;
        std::size_t mid = 0;
        for ( const auto& row : tiles.tileFlags[mega].miniTileFlags )
        {
            for ( const auto& mini : row )
            {
                anyWalkable = anyWalkable || mini.isWalkable();
                allWalkable = allWalkable && mini.isWalkable();
                ramp = ramp || mini.isRamp();
                blocksView = blocksView || mini.blocksView();
                high += mini.getElevation() == Sc::Terrain::TileElevation::High ? 1 : 0;
                mid += mini.getElevation() == Sc::Terrain::TileElevation::Mid ? 1 : 0;
            }
        }
        result.emplace("walkability", allWalkable ? "all" : (anyWalkable ? "any" : "none"));
        result.emplace("ramp", ramp);
        result.emplace("blocksView", blocksView);
        result.emplace("highMinitiles", high);
        result.emplace("midMinitiles", mid);
    }
    const std::string fingerprint = sha256Bytes(&record, sizeof(record)) + ":" + std::to_string(variant);
    result.emplace("fingerprint", fingerprint);
    return result;
}

struct CatalogFilter {
    std::optional<std::size_t> id;
    std::optional<std::size_t> terrainType;
    std::optional<std::size_t> group;
    std::optional<std::size_t> variant;
    std::optional<bool> graphicsValid;
    std::optional<std::string> walkability;
    std::optional<std::size_t> groundHeight;
    std::optional<std::size_t> buildability;
    std::optional<bool> ramp;
    std::optional<bool> blocksView;
    std::optional<bool> overlay;
    std::optional<bool> visible;
    std::optional<std::size_t> width;
    std::optional<std::size_t> height;
    std::optional<std::size_t> placementWidth;
    std::optional<std::size_t> placementHeight;
};

std::optional<std::size_t> catalogSizeFilter(const Json::Object& filter, const char* field, std::size_t maximum)
{
    const Json* value = optionalField(filter, field);
    if ( value == nullptr ) return std::nullopt;
    const std::size_t number = sizeValue(*value, "catalog filter." + std::string(field));
    if ( number > maximum ) fail("catalog filter." + std::string(field) + ": value exceeds supported range");
    return number;
}

std::optional<bool> catalogBoolFilter(const Json::Object& filter, const char* field)
{
    const Json* value = optionalField(filter, field);
    return value == nullptr ? std::nullopt
                            : std::optional<bool>(boolValue(*value, "catalog filter." + std::string(field)));
}

CatalogFilter parseCatalogFilter(const Json::Object& request, const std::string& kind)
{
    const Json* value = optionalField(request, "filter");
    if ( value == nullptr ) return {};
    const auto& input = objectValue(*value, "catalog filter");
    std::set<std::string> allowed{"id"};
    if ( kind == "tiles" )
        allowed.insert({"terrainType", "group", "variant", "graphicsValid", "walkability", "groundHeight",
            "buildability", "ramp", "blocksView"});
    else if ( kind == "brushes" )
        allowed.insert({"terrainType", "graphicsValid", "walkability", "groundHeight", "buildability", "ramp", "blocksView"});
    else if ( kind == "units" || kind == "buildings" )
        allowed.insert({"placementWidth", "placementHeight"});
    else if ( kind == "doodads" )
        allowed.insert({"graphicsValid", "overlay", "width", "height", "buildability"});
    else if ( kind == "sprites" )
        allowed.insert({"visible"});
    else
        fail("unsupported catalog kind: " + kind);
    for ( const auto& entry : input )
    {
        if ( allowed.find(entry.first) == allowed.end() )
            fail("catalog filter." + entry.first + " is not supported for " + kind);
    }

    constexpr std::size_t MaxU16 = std::numeric_limits<std::uint16_t>::max();
    CatalogFilter filter;
    filter.id = catalogSizeFilter(input, "id", MaxU16);
    filter.terrainType = catalogSizeFilter(input, "terrainType", MaxU16);
    filter.group = catalogSizeFilter(input, "group", Sc::Terrain::Cv5Dat::MaxTileGroups - 1);
    filter.variant = catalogSizeFilter(input, "variant", 15);
    filter.graphicsValid = catalogBoolFilter(input, "graphicsValid");
    if ( const Json* walkability = optionalField(input, "walkability"); walkability != nullptr )
    {
        const std::string& name = stringValue(*walkability, "catalog filter.walkability");
        if ( name != "all" && name != "any" && name != "none" )
            fail("catalog filter.walkability must be all, any, or none");
        filter.walkability = name;
    }
    filter.groundHeight = catalogSizeFilter(input, "groundHeight", MaxU16);
    filter.buildability = catalogSizeFilter(input, "buildability", MaxU16);
    filter.ramp = catalogBoolFilter(input, "ramp");
    filter.blocksView = catalogBoolFilter(input, "blocksView");
    filter.overlay = catalogBoolFilter(input, "overlay");
    filter.visible = catalogBoolFilter(input, "visible");
    filter.width = catalogSizeFilter(input, "width", MaxU16);
    filter.height = catalogSizeFilter(input, "height", MaxU16);
    filter.placementWidth = catalogSizeFilter(input, "placementWidth", MaxU16);
    filter.placementHeight = catalogSizeFilter(input, "placementHeight", MaxU16);
    return filter;
}

bool catalogSizeMatches(const Json::Object& entry, const char* field, const std::optional<std::size_t>& expected)
{
    if ( !expected.has_value() ) return true;
    const Json* value = optionalField(entry, field);
    if ( value == nullptr ) return false;
    const auto* integer = std::get_if<std::int64_t>(&value->value);
    return integer != nullptr && *integer >= 0 && static_cast<std::size_t>(*integer) == *expected;
}

bool catalogBoolMatches(const Json::Object& entry, const char* field, const std::optional<bool>& expected)
{
    if ( !expected.has_value() ) return true;
    const Json* value = optionalField(entry, field);
    if ( value == nullptr ) return false;
    const auto* boolean = std::get_if<bool>(&value->value);
    return boolean != nullptr && *boolean == *expected;
}

bool catalogStringMatches(const Json::Object& entry, const char* field, const std::optional<std::string>& expected)
{
    if ( !expected.has_value() ) return true;
    const Json* value = optionalField(entry, field);
    if ( value == nullptr ) return false;
    const auto* string = std::get_if<std::string>(&value->value);
    return string != nullptr && *string == *expected;
}

bool catalogFilterMatches(const Json::Object& entry, const CatalogFilter& filter)
{
    return catalogSizeMatches(entry, "id", filter.id) &&
           catalogSizeMatches(entry, "terrainType", filter.terrainType) &&
           catalogSizeMatches(entry, "group", filter.group) &&
           catalogSizeMatches(entry, "variant", filter.variant) &&
           catalogBoolMatches(entry, "graphicsValid", filter.graphicsValid) &&
           catalogStringMatches(entry, "walkability", filter.walkability) &&
           catalogSizeMatches(entry, "groundHeight", filter.groundHeight) &&
           catalogSizeMatches(entry, "buildability", filter.buildability) &&
           catalogBoolMatches(entry, "ramp", filter.ramp) &&
           catalogBoolMatches(entry, "blocksView", filter.blocksView) &&
           catalogBoolMatches(entry, "overlay", filter.overlay) &&
           catalogBoolMatches(entry, "visible", filter.visible) &&
           catalogSizeMatches(entry, "width", filter.width) &&
           catalogSizeMatches(entry, "height", filter.height) &&
           catalogSizeMatches(entry, "placementWidth", filter.placementWidth) &&
           catalogSizeMatches(entry, "placementHeight", filter.placementHeight);
}

void renderThumbnail(const Json::Object& request, Sc::Terrain::Tileset tileset, Assets& assets,
    std::vector<std::uint8_t>& rgba, std::uint32_t& width, std::uint32_t& height)
{
    allowedFields(request, {"schema", "mode", "layer", "id", "owner", "tileset"}, "render request");
    const std::string layer = stringValue(requiredField(request, "layer", "render request"), "render request.layer");
    const std::size_t id = checkedSize(request, "id", "render request");
    const std::uint8_t owner = optionalU8(request, "owner", 0, "render request");
    const auto& tiles = assets.terrain.get(tileset);
    width = 96;
    height = 96;
    rgba.assign(static_cast<std::size_t>(width) * height * 4, 0);
    for ( std::size_t y = 0; y < height; ++y )
    {
        for ( std::size_t x = 0; x < width; ++x )
        {
            const std::size_t offset = (y * width + x) * 4;
            rgba[offset] = 17;
            rgba[offset + 1] = 24;
            rgba[offset + 2] = 39;
            rgba[offset + 3] = 255;
        }
    }
    if ( layer == "terrain" )
    {
        if ( id > std::numeric_limits<std::uint16_t>::max() || !tileGraphicsValid(tiles, static_cast<std::uint16_t>(id)) )
            fail("thumbnail tile is invalid");
        if ( tileset == Sc::Terrain::Tileset::SpacePlatform )
        {
            if ( assets.starLayers.empty() )
                fail("cannot load StarCraft Space Platform star parallax");
            drawStarParallax(rgba, width, height, 0, 0, 1, assets, tiles);
        }
        for ( std::size_t y = 0; y < height; ++y )
        {
            for ( std::size_t x = 0; x < width; ++x )
            {
                putTerrainPixel(
                    rgba,
                    width,
                    x,
                    y,
                    static_cast<std::uint16_t>(id),
                    x * 32 / static_cast<std::size_t>(width),
                    y * 32 / static_cast<std::size_t>(height),
                    tiles);
            }
        }
        return;
    }
    if ( layer == "units" || layer == "buildings" )
    {
        const std::uint16_t sprite = unitSpriteId(assets, static_cast<std::uint16_t>(id));
        if ( sprite == std::numeric_limits<std::uint16_t>::max() ) fail("thumbnail unit is invalid");
        const std::uint8_t direction = assets.units.getUnit(Sc::Unit::Type(static_cast<std::uint16_t>(id))).unitDirection;
        drawGrp(rgba, width, height, 0, 0, 1, sprite, owner, direction, 48, 48, assets, tiles);
        return;
    }
    if ( layer == "sprites" )
    {
        if ( id >= Sc::Sprite::TotalSprites ) fail("thumbnail sprite is invalid");
        drawGrp(rgba, width, height, 0, 0, 1, static_cast<std::uint16_t>(id), owner, 0, 48, 48, assets, tiles);
        return;
    }
    if ( layer == "doodads" )
    {
        const auto& doodads = assets.doodads(tileset);
        if ( id >= doodads.size() ) fail("thumbnail doodad is invalid");
        const auto& record = doodads[id];
        const std::size_t startX = (96 - std::min<std::size_t>(record.doodadWidth * 32, 96)) / 2;
        const std::size_t startY = (96 - std::min<std::size_t>(record.doodadHeight * 32, 96)) / 2;
        for ( std::size_t y = 0; y < record.doodadHeight && y < 3; ++y )
        {
            for ( std::size_t x = 0; x < record.doodadWidth && x < 3; ++x )
            {
                const std::uint16_t tile = static_cast<std::uint16_t>((Sc::Terrain::Cv5Dat::MaxTileGroups + id) * 16 + y * record.doodadWidth + x);
                if ( !tileGraphicsValid(tiles, tile) ) continue;
                for ( std::size_t py = 0; py < 32 && startY + y * 32 + py < height; ++py )
                    for ( std::size_t px = 0; px < 32 && startX + x * 32 + px < width; ++px )
                        putTerrainPixel(rgba, width, startX + x * 32 + px, startY + y * 32 + py, tile, px, py, tiles);
            }
        }
        if ( (record.flags & 0x30) != 0 && record.overlayIndex < Sc::Sprite::TotalSprites )
            drawGrp(rgba, width, height, 0, 0, 1, record.overlayIndex, owner, 0, 48, 48, assets, tiles);
        return;
    }
    fail("unsupported thumbnail layer: " + layer);
}

struct ImageTileColor {
    std::uint8_t red;
    std::uint8_t green;
    std::uint8_t blue;
    std::uint8_t walkability;
    std::uint8_t height;
    std::uint16_t tile;
    bool paletteEligible;
};

std::optional<ImageTileColor> imageTileColor(
    const Sc::Terrain::Tiles& tiles,
    std::uint16_t tile)
{
    if ( !tileGraphicsValid(tiles, tile) )
        return std::nullopt;
    const std::size_t group = static_cast<std::size_t>(tile) / 16;
    const std::size_t mega = tiles.tileGroups[group].megaTileIndex[tile % 16];
    const auto& graphics = tiles.tileGraphics[mega];
    std::uint64_t red = 0;
    std::uint64_t green = 0;
    std::uint64_t blue = 0;
    std::uint64_t samples = 0;
    for ( std::size_t y = 0; y < 32; ++y )
    {
        for ( std::size_t x = 0; x < 32; ++x )
        {
            const auto& mini = graphics.miniTileGraphics[y / 8][x / 8];
            std::size_t subX = x % 8;
            if ( mini.isFlipped() )
                subX = 7 - subX;
            const std::uint8_t palette =
                tiles.miniTilePixels[mini.vr4Index()].wpeIndex[y % 8][subX];
            if ( palette == 0 )
                continue;
            const auto& color = tiles.systemColorPalette[palette];
            red += color.red;
            green += color.green;
            blue += color.blue;
            ++samples;
        }
    }
    const bool paletteEligible = samples != 0;
    bool anyWalkable = false;
    bool allWalkable = true;
    for ( const auto& row : tiles.tileFlags[mega].miniTileFlags )
    {
        for ( const auto& mini : row )
        {
            anyWalkable = anyWalkable || mini.isWalkable();
            allWalkable = allWalkable && mini.isWalkable();
        }
    }
    const auto rounded = [samples](std::uint64_t value) {
        return static_cast<std::uint8_t>((value + samples / 2) / samples);
    };
    const auto& transparent = tiles.systemColorPalette[0];
    return ImageTileColor{
        paletteEligible ? rounded(red) : transparent.red,
        paletteEligible ? rounded(green) : transparent.green,
        paletteEligible ? rounded(blue) : transparent.blue,
        static_cast<std::uint8_t>(allWalkable ? 2 : (anyWalkable ? 1 : 0)),
        static_cast<std::uint8_t>(tiles.tileGroups[group].groundHeight),
        tile,
        paletteEligible,
    };
}

struct ImagePaletteNode {
    std::size_t entry;
    std::size_t lower;
    std::size_t upper;
    std::uint8_t axis;
};

struct ImagePalette {
    std::vector<ImageTileColor> entries;
    std::vector<ImagePaletteNode> nodes;
    std::size_t root = std::numeric_limits<std::size_t>::max();
};

std::uint8_t imageColorChannel(const ImageTileColor& color, std::size_t axis)
{
    return axis == 0 ? color.red : (axis == 1 ? color.green : color.blue);
}

std::size_t buildImagePaletteTree(
    ImagePalette& palette,
    std::vector<std::size_t>& indices,
    std::size_t begin,
    std::size_t end,
    std::size_t depth)
{
    if ( begin == end )
        return std::numeric_limits<std::size_t>::max();
    const std::size_t axis = depth % 3;
    std::stable_sort(indices.begin() + begin, indices.begin() + end,
        [&palette, axis](std::size_t left, std::size_t right) {
            const auto leftChannel = imageColorChannel(palette.entries[left], axis);
            const auto rightChannel = imageColorChannel(palette.entries[right], axis);
            return leftChannel == rightChannel ? left < right : leftChannel < rightChannel;
        });
    const std::size_t middle = begin + (end - begin) / 2;
    const std::size_t node = palette.nodes.size();
    palette.nodes.push_back(ImagePaletteNode{
        indices[middle],
        std::numeric_limits<std::size_t>::max(),
        std::numeric_limits<std::size_t>::max(),
        static_cast<std::uint8_t>(axis),
    });
    palette.nodes[node].lower =
        buildImagePaletteTree(palette, indices, begin, middle, depth + 1);
    palette.nodes[node].upper =
        buildImagePaletteTree(palette, indices, middle + 1, end, depth + 1);
    return node;
}

void nearestImageTile(
    const ImagePalette& palette,
    std::size_t node,
    const std::array<std::uint8_t, 3>& rgb,
    std::size_t& best,
    std::uint32_t& bestDistance)
{
    if ( node == std::numeric_limits<std::size_t>::max() )
        return;
    const auto& treeNode = palette.nodes[node];
    const auto& entry = palette.entries[treeNode.entry];
    const std::int32_t red = static_cast<std::int32_t>(rgb[0]) - entry.red;
    const std::int32_t green = static_cast<std::int32_t>(rgb[1]) - entry.green;
    const std::int32_t blue = static_cast<std::int32_t>(rgb[2]) - entry.blue;
    const std::uint32_t distance =
        static_cast<std::uint32_t>(red * red + green * green + blue * blue);
    if ( distance < bestDistance || (distance == bestDistance && treeNode.entry < best) )
    {
        best = treeNode.entry;
        bestDistance = distance;
    }

    const std::int32_t delta =
        static_cast<std::int32_t>(rgb[treeNode.axis])
        - imageColorChannel(entry, treeNode.axis);
    const std::size_t nearNode = delta < 0 ? treeNode.lower : treeNode.upper;
    const std::size_t farNode = delta < 0 ? treeNode.upper : treeNode.lower;
    nearestImageTile(palette, nearNode, rgb, best, bestDistance);
    if ( static_cast<std::uint32_t>(delta * delta) <= bestDistance )
        nearestImageTile(palette, farNode, rgb, best, bestDistance);
}

std::shared_ptr<const ImagePalette> imagePalette(
    const std::string& root,
    const Assets& assets,
    Sc::Terrain::Tileset tileset)
{
    static std::mutex cacheMutex;
    static std::map<std::string, std::shared_ptr<const ImagePalette>> cache;
    const std::string key =
        lowerAscii(root) + ":" + std::to_string(static_cast<std::size_t>(tileset));
    std::lock_guard<std::mutex> guard(cacheMutex);
    const auto found = cache.find(key);
    if ( found != cache.end() )
        return found->second;

    const auto& tiles = assets.terrain.get(tileset);
    const std::size_t total =
        std::min<std::size_t>(Sc::Terrain::Cv5Dat::MaxTileGroups, tiles.tileGroups.size()) * 16;
    std::map<std::uint32_t, std::size_t> seenColors;
    auto palette = std::make_shared<ImagePalette>();
    palette->entries.reserve(total);
    for ( std::size_t scanOrder = 0; scanOrder < total; ++scanOrder )
    {
        const auto color = imageTileColor(tiles, static_cast<std::uint16_t>(scanOrder));
        if ( !color.has_value() || !color->paletteEligible )
            continue;
        const std::uint32_t rgb =
            (static_cast<std::uint32_t>(color->red) << 16)
            | (static_cast<std::uint32_t>(color->green) << 8)
            | color->blue;
        if ( seenColors.emplace(rgb, scanOrder).second )
            palette->entries.push_back(*color);
    }
    if ( palette->entries.empty() )
        fail("image quantizer palette has no graphics-valid terrain tiles");
    std::vector<std::size_t> indices(palette->entries.size());
    for ( std::size_t index = 0; index < indices.size(); ++index )
        indices[index] = index;
    palette->nodes.reserve(indices.size());
    palette->root = buildImagePaletteTree(*palette, indices, 0, indices.size(), 0);
    cache.emplace(key, palette);
    return palette;
}

void appendU16(std::vector<std::uint8_t>& output, std::uint16_t value)
{
    output.push_back(static_cast<std::uint8_t>(value & 0xff));
    output.push_back(static_cast<std::uint8_t>(value >> 8));
}

void appendU32(std::vector<std::uint8_t>& output, std::uint32_t value)
{
    output.push_back(static_cast<std::uint8_t>(value & 0xff));
    output.push_back(static_cast<std::uint8_t>((value >> 8) & 0xff));
    output.push_back(static_cast<std::uint8_t>((value >> 16) & 0xff));
    output.push_back(static_cast<std::uint8_t>((value >> 24) & 0xff));
}

} // namespace

int mapEdit(const char* inputMapPath, const char* outputMapPath, const char* starCraftPath,
    const std::uint8_t* batchJson, std::size_t batchLength, std::string& reportJson)
{
    if ( inputMapPath == nullptr || outputMapPath == nullptr || starCraftPath == nullptr ||
         starCraftPath[0] == '\0' || batchJson == nullptr || batchLength == 0 )
        fail("mapedit received an invalid argument");
    const std::string inputPath(inputMapPath);
    const std::string outputPath(outputMapPath);
    if ( inputPath.empty() || outputPath.empty() || lowerAscii(inputPath) == lowerAscii(outputPath) )
        fail("mapedit requires distinct non-empty input and output paths");
    ::DeleteFileA(outputPath.c_str());

    const Json root = parseJson(std::string(reinterpret_cast<const char*>(batchJson), batchLength), "map edit batch");
    const auto& object = objectValue(root, "map edit batch");
    exactFields(object, {"schema", "expected", "operations"}, "map edit batch");
    if ( stringValue(requiredField(object, "schema", "map edit batch"), "map edit batch.schema") != EditSchema )
        fail("unsupported map edit schema");
    const auto& expected = objectValue(requiredField(object, "expected", "map edit batch"), "map edit batch.expected");
    exactFields(expected, {"inputFileSha256", "tileset", "width", "height"}, "map edit batch.expected");
    const std::string expectedHash = stringValue(requiredField(expected, "inputFileSha256", "map edit batch.expected"), "expected.inputFileSha256");
    if ( readFileSha256(inputPath) != lowerAscii(expectedHash) )
        fail("input map sha256 does not match batch expected authority");

    MapFile map(inputPath);
    if ( map.empty() ) fail("cannot load input map: " + inputPath);
    const auto expectedTileset = parseTileset(stringValue(requiredField(expected, "tileset", "map edit batch.expected"), "expected.tileset"));
    if ( map.getTileset() != expectedTileset ) fail("input map tileset does not match batch expected authority");
    if ( map.getTileWidth() != checkedSize(expected, "width", "map edit batch.expected", true) ||
         map.getTileHeight() != checkedSize(expected, "height", "map edit batch.expected", true) )
        fail("input map dimensions do not match batch expected authority");

    const auto& operations = arrayValue(requiredField(object, "operations", "map edit batch"), "map edit batch.operations");
    if ( operations.empty() || operations.size() > MaxOperations ) fail("map edit batch operation count is out of range");
    const auto assets = loadAssets(starCraftPath);
    const auto& terrainTiles = assets->terrain.get(map.getTileset());
    Json::Array effects;

    for ( std::size_t index = 0; index < operations.size(); ++index )
    {
        const std::string context = "map edit batch.operations[" + std::to_string(index) + "]";
        const auto& operation = objectValue(operations[index], context);
        const std::string name = stringValue(requiredField(operation, "op", context), context + ".op");
        if ( name == "terrain.set" )
        {
            exactFields(operation, {"op", "x", "y", "before", "after"}, context);
            const std::size_t x = checkedSize(operation, "x", context);
            const std::size_t y = checkedSize(operation, "y", context);
            const std::uint16_t before = checkedU16(operation, "before", context);
            const std::uint16_t after = checkedU16(operation, "after", context);
            if ( x >= map.getTileWidth() || y >= map.getTileHeight() ) fail(context + ": terrain coordinate is outside map");
            if ( map.getTile(x, y) != before ) fail(context + ": terrain expected-before conflict");
            if ( !tileGraphicsValid(terrainTiles, after) ) fail(context + ": terrain tile has invalid graphics");
            setExactTile(map, x, y, after);
            effects.emplace_back(effect(name, "terrain", y * map.getTileWidth() + x));
        }
        else if ( name == "terrain.rect" )
        {
            exactFields(operation, {"op", "x", "y", "width", "height", "after"}, context);
            const std::size_t x = checkedSize(operation, "x", context);
            const std::size_t y = checkedSize(operation, "y", context);
            const std::size_t width = checkedSize(operation, "width", context, true);
            const std::size_t height = checkedSize(operation, "height", context, true);
            const std::uint16_t after = checkedU16(operation, "after", context);
            if ( x + width > map.getTileWidth() || y + height > map.getTileHeight() ) fail(context + ": terrain rectangle is outside map");
            if ( !tileGraphicsValid(terrainTiles, after) ) fail(context + ": terrain tile has invalid graphics");
            for ( std::size_t row = 0; row < height; ++row )
                for ( std::size_t column = 0; column < width; ++column )
                    setExactTile(map, x + column, y + row, after);
            effects.emplace_back(effect(name, "terrain", width * height));
        }
        else if ( name == "terrain.blit" )
        {
            exactFields(operation, {"op", "x", "y", "tiles"}, context);
            const std::size_t x = checkedSize(operation, "x", context);
            const std::size_t y = checkedSize(operation, "y", context);
            const auto& rows = arrayValue(requiredField(operation, "tiles", context), context + ".tiles");
            if ( rows.empty() ) fail(context + ": terrain blit cannot be empty");
            std::size_t width = 0;
            std::vector<std::vector<std::uint16_t>> parsed;
            for ( std::size_t rowIndex = 0; rowIndex < rows.size(); ++rowIndex )
            {
                const auto& row = arrayValue(rows[rowIndex], context + ".tiles[]");
                if ( row.empty() || (width != 0 && row.size() != width) ) fail(context + ": terrain blit rows must be non-empty and rectangular");
                width = row.size();
                std::vector<std::uint16_t> values;
                for ( const auto& value : row )
                {
                    const auto number = sizeValue(value, context + ".tiles[][]");
                    if ( number > std::numeric_limits<std::uint16_t>::max() || !tileGraphicsValid(terrainTiles, static_cast<std::uint16_t>(number)) )
                        fail(context + ": terrain blit contains invalid tile graphics");
                    values.push_back(static_cast<std::uint16_t>(number));
                }
                parsed.push_back(std::move(values));
            }
            if ( x + width > map.getTileWidth() || y + parsed.size() > map.getTileHeight() ) fail(context + ": terrain blit is outside map");
            for ( std::size_t row = 0; row < parsed.size(); ++row )
                for ( std::size_t column = 0; column < width; ++column )
                    setExactTile(map, x + column, y + row, parsed[row][column]);
            effects.emplace_back(effect(name, "terrain", width * parsed.size()));
        }
        else if ( name == "terrain.isom_brush" )
        {
            exactFields(operation, {"op", "isomX", "isomY", "brush", "extent"}, context);
            ScMap scMap = copyToScMap(map);
            Chk::IsomCache cache(map.getTileset(), map.getTileWidth(), map.getTileHeight(), assets->isom.get(map.getTileset()));
            const std::size_t isomX = checkedSize(operation, "isomX", context);
            const std::size_t isomY = checkedSize(operation, "isomY", context);
            const std::size_t brush = checkedSize(operation, "brush", context, true);
            const std::size_t extent = checkedSize(operation, "extent", context, true);
            if ( !scMap.placeIsomTerrain({isomX, isomY}, brush, extent, cache) ) fail(context + ": semantic ISOM brush placement failed");
            scMap.updateTilesFromIsom(cache);
            copyFromScMap(map, scMap);
            effects.emplace_back(effect(name, "terrain", index));
        }
        else if ( name == "unit.add" )
        {
            exactFields(operation, {"op", "state"}, context);
            const Chk::Unit unit = parseUnitState(requiredField(operation, "state", context), context + ".state");
            requirePosition(unit.xc, unit.yc, map, context);
            const std::size_t ordinal = map.addUnit(unit);
            effects.emplace_back(effect(name, "unit", ordinal));
        }
        else if ( name == "unit.set" || name == "unit.move" || name == "unit.delete" )
        {
            const std::set<std::string> fields = name == "unit.set" ? std::set<std::string>{"op", "ordinal", "beforeFingerprint", "state"}
                : (name == "unit.move" ? std::set<std::string>{"op", "ordinal", "beforeFingerprint", "x", "y"}
                                       : std::set<std::string>{"op", "ordinal", "beforeFingerprint"});
            exactFields(operation, fields, context);
            const std::size_t ordinal = checkedSize(operation, "ordinal", context);
            if ( ordinal >= map.numUnits() ) fail(context + ": unit ordinal is out of range");
            if ( unitFingerprint(map.getUnit(ordinal)) != stringValue(requiredField(operation, "beforeFingerprint", context), context + ".beforeFingerprint") )
                fail(context + ": unit expected-before fingerprint conflict");
            if ( name == "unit.delete" ) map.deleteUnit(ordinal);
            else if ( name == "unit.move" )
            {
                const std::uint16_t x = checkedU16(operation, "x", context);
                const std::uint16_t y = checkedU16(operation, "y", context);
                requirePosition(x, y, map, context);
                map.getUnit(ordinal).xc = x;
                map.getUnit(ordinal).yc = y;
            }
            else
            {
                const auto& state = objectValue(requiredField(operation, "state", context), context + ".state");
                allowedFields(state, {"typeId", "owner", "x", "y", "classId", "relationFlags", "validStateFlags",
                    "validFieldFlags", "hpPercent", "shieldPercent", "energyPercent", "resourceAmount", "hangarAmount",
                    "stateFlags", "unused", "relationClassId"}, context + ".state");
                if ( state.empty() ) fail(context + ": unit.set state cannot be empty");
                Chk::Unit& unit = map.getUnit(ordinal);
                if ( optionalField(state, "typeId") != nullptr )
                {
                    const std::uint16_t typeId = checkedU16(state, "typeId", context + ".state");
                    if ( typeId >= Sc::Unit::TotalTypes ) fail(context + ": unit type is out of range");
                    unit.type = Sc::Unit::Type(typeId);
                }
                if ( optionalField(state, "owner") != nullptr ) unit.owner = optionalU8(state, "owner", 0, context + ".state");
                if ( optionalField(state, "x") != nullptr ) unit.xc = checkedU16(state, "x", context + ".state");
                if ( optionalField(state, "y") != nullptr ) unit.yc = checkedU16(state, "y", context + ".state");
                if ( optionalField(state, "classId") != nullptr ) unit.classId = optionalU32(state, "classId", 0, context + ".state");
                if ( optionalField(state, "relationFlags") != nullptr ) unit.relationFlags = optionalU16(state, "relationFlags", 0, context + ".state");
                if ( optionalField(state, "validStateFlags") != nullptr ) unit.validStateFlags = optionalU16(state, "validStateFlags", 0, context + ".state");
                if ( optionalField(state, "validFieldFlags") != nullptr ) unit.validFieldFlags = optionalU16(state, "validFieldFlags", 0, context + ".state");
                if ( optionalField(state, "hpPercent") != nullptr ) unit.hitpointPercent = optionalU8(state, "hpPercent", 100, context + ".state");
                if ( optionalField(state, "shieldPercent") != nullptr ) unit.shieldPercent = optionalU8(state, "shieldPercent", 100, context + ".state");
                if ( optionalField(state, "energyPercent") != nullptr ) unit.energyPercent = optionalU8(state, "energyPercent", 100, context + ".state");
                if ( optionalField(state, "resourceAmount") != nullptr ) unit.resourceAmount = optionalU32(state, "resourceAmount", 0, context + ".state");
                if ( optionalField(state, "hangarAmount") != nullptr ) unit.hangerAmount = optionalU16(state, "hangarAmount", 0, context + ".state");
                if ( optionalField(state, "stateFlags") != nullptr ) unit.stateFlags = optionalU16(state, "stateFlags", 0, context + ".state");
                if ( optionalField(state, "unused") != nullptr ) unit.unused = optionalU32(state, "unused", 0, context + ".state");
                if ( optionalField(state, "relationClassId") != nullptr ) unit.relationClassId = optionalU32(state, "relationClassId", 0, context + ".state");
                if ( unit.owner > 11 ) fail(context + ": owner must be 0..11");
                if ( unit.hitpointPercent > 100 || unit.shieldPercent > 100 || unit.energyPercent > 100 )
                    fail(context + ": hp/shield/energy percentages must be 0..100");
                requirePosition(unit.xc, unit.yc, map, context);
            }
            effects.emplace_back(effect(name, "unit", ordinal));
        }
        else if ( name == "sprite.add" )
        {
            exactFields(operation, {"op", "state"}, context);
            const Chk::Sprite sprite = parseSpriteState(requiredField(operation, "state", context), context + ".state");
            requirePosition(sprite.xc, sprite.yc, map, context);
            const std::size_t ordinal = map.addSprite(sprite);
            effects.emplace_back(effect(name, "sprite", ordinal));
        }
        else if ( name == "sprite.set" || name == "sprite.move" || name == "sprite.delete" )
        {
            const std::set<std::string> fields = name == "sprite.set" ? std::set<std::string>{"op", "ordinal", "beforeFingerprint", "state"}
                : (name == "sprite.move" ? std::set<std::string>{"op", "ordinal", "beforeFingerprint", "x", "y"}
                                         : std::set<std::string>{"op", "ordinal", "beforeFingerprint"});
            exactFields(operation, fields, context);
            const std::size_t ordinal = checkedSize(operation, "ordinal", context);
            if ( ordinal >= map.numSprites() ) fail(context + ": sprite ordinal is out of range");
            if ( spriteFingerprint(map.getSprite(ordinal)) != stringValue(requiredField(operation, "beforeFingerprint", context), context + ".beforeFingerprint") )
                fail(context + ": sprite expected-before fingerprint conflict");
            if ( name == "sprite.delete" ) map.deleteSprite(ordinal);
            else if ( name == "sprite.move" )
            {
                const std::uint16_t x = checkedU16(operation, "x", context);
                const std::uint16_t y = checkedU16(operation, "y", context);
                requirePosition(x, y, map, context);
                map.getSprite(ordinal).xc = x;
                map.getSprite(ordinal).yc = y;
            }
            else
            {
                const Chk::Sprite sprite = parseSpriteState(requiredField(operation, "state", context), context + ".state");
                requirePosition(sprite.xc, sprite.yc, map, context);
                map.getSprite(ordinal) = sprite;
            }
            effects.emplace_back(effect(name, "sprite", ordinal));
        }
        else if ( name == "doodad.add" )
        {
            exactFields(operation, {"op", "state"}, context);
            const Chk::Doodad doodad = parseDoodadState(requiredField(operation, "state", context), context + ".state");
            requirePosition(doodad.xc, doodad.yc, map, context);
            applyDoodadFootprint(map, doodad, *assets, context);
            const std::size_t ordinal = map.addDoodad(doodad);
            addDoodadOverlay(map, doodad, *assets, context);
            effects.emplace_back(effect(name, "doodad", ordinal));
        }
        else if ( name == "doodad.set" || name == "doodad.move" || name == "doodad.delete" )
        {
            const std::set<std::string> fields = name == "doodad.set" ? std::set<std::string>{"op", "ordinal", "beforeFingerprint", "state", "replacementTiles"}
                : (name == "doodad.move" ? std::set<std::string>{"op", "ordinal", "beforeFingerprint", "x", "y", "replacementTiles"}
                                         : std::set<std::string>{"op", "ordinal", "beforeFingerprint", "replacementTiles"});
            exactFields(operation, fields, context);
            const std::size_t ordinal = checkedSize(operation, "ordinal", context);
            if ( ordinal >= map.numDoodads() ) fail(context + ": doodad ordinal is out of range");
            const Chk::Doodad before = map.getDoodad(ordinal);
            if ( doodadFingerprint(before) != stringValue(requiredField(operation, "beforeFingerprint", context), context + ".beforeFingerprint") )
                fail(context + ": doodad expected-before fingerprint conflict");
            replaceDoodadFootprint(map, before,
                requiredField(operation, "replacementTiles", context), *assets, context);
            deleteDoodadOverlay(map, before, *assets, context);
            if ( name == "doodad.delete" ) map.deleteDoodad(ordinal);
            else
            {
                Chk::Doodad after = before;
                if ( name == "doodad.move" )
                {
                    after.xc = checkedU16(operation, "x", context);
                    after.yc = checkedU16(operation, "y", context);
                }
                else
                    after = parseDoodadState(requiredField(operation, "state", context), context + ".state");
                requirePosition(after.xc, after.yc, map, context);
                applyDoodadFootprint(map, after, *assets, context);
                map.getDoodad(ordinal) = after;
                addDoodadOverlay(map, after, *assets, context);
            }
            effects.emplace_back(effect(name, "doodad", ordinal));
        }
        else if ( name == "location.add" || name == "location.set" )
        {
            exactFields(operation, {"op", "state"}, context);
            const auto& state = objectValue(requiredField(operation, "state", context), context + ".state");
            allowedFields(state, {"locationId", "left", "top", "right", "bottom", "elevationFlags", "nameBytesHex"}, context + ".state");
            Chk::Location location{};
            location.left = checkedU32(state, "left", context + ".state");
            location.top = checkedU32(state, "top", context + ".state");
            location.right = checkedU32(state, "right", context + ".state");
            location.bottom = checkedU32(state, "bottom", context + ".state");
            location.elevationFlags = optionalU16(state, "elevationFlags", 0, context + ".state");
            std::size_t id = checkedSize(state, "locationId", context + ".state");
            if ( name == "location.add" )
            {
                if ( id != 0 ) fail(context + ": location.add locationId must be 0 (assigned by map)");
                id = map.addLocation(location);
                if ( id == static_cast<std::size_t>(Chk::LocationId::NoLocation) ) fail(context + ": no free location slot");
            }
            else
            {
                requireLocationId(map, id, context);
                Chk::Location& existing = map.getLocation(id);
                if ( blankLocation(existing) ) fail(context + ": location slot is empty");
                location.stringId = existing.stringId;
                existing = location;
            }
            if ( const Json* nameHex = optionalField(state, "nameBytesHex") )
                map.setLocationName<RawString>(id, RawString(rawStringFromHex(stringValue(*nameHex, context + ".state.nameBytesHex"), context + ".state.nameBytesHex")), Chk::StrScope::Game, false);
            effects.emplace_back(effect(name, "location", id));
        }
        else if ( name == "location.rename" )
        {
            exactFields(operation, {"op", "locationId", "nameBytesHex"}, context);
            const std::size_t id = checkedSize(operation, "locationId", context);
            requireLocationId(map, id, context);
            if ( blankLocation(map.getLocation(id)) ) fail(context + ": location slot is empty");
            const std::string raw = rawStringFromHex(stringValue(requiredField(operation, "nameBytesHex", context), context + ".nameBytesHex"), context + ".nameBytesHex");
            map.setLocationName<RawString>(id, RawString(raw), Chk::StrScope::Game, false);
            effects.emplace_back(effect(name, "location", id));
        }
        else if ( name == "location.delete" )
        {
            exactFields(operation, {"op", "locationId"}, context);
            const std::size_t id = checkedSize(operation, "locationId", context);
            requireLocationId(map, id, context);
            if ( blankLocation(map.getLocation(id)) ) fail(context + ": location slot is empty");
            map.deleteLocation(id, true);
            if ( !blankLocation(map.getLocation(id)) ) fail(context + ": location is referenced by a trigger and cannot be deleted");
            effects.emplace_back(effect(name, "location", id));
        }
        else
            fail(context + ": unsupported operation '" + name + "'");
    }

    const AssetInventory beforeAssets = inventoryMpq(inputPath);
    const std::string temporary = temporaryOutputPath(outputPath);
    ::DeleteFileA(temporary.c_str());
    if ( !map.save(temporary, true, true, true, false) )
    {
        ::DeleteFileA(temporary.c_str());
        fail("map save failed before output promotion");
    }
    const AssetInventory afterAssets = inventoryMpq(temporary);
    if ( beforeAssets != afterAssets )
    {
        ::DeleteFileA(temporary.c_str());
        fail("map save changed unrelated MPQ assets");
    }
    if ( !replaceFile(temporary, outputPath) )
    {
        ::DeleteFileA(temporary.c_str());
        fail("map output atomic promotion failed");
    }
    MapFile verified(outputPath);
    if ( verified.empty() )
    {
        ::DeleteFileA(outputPath.c_str());
        fail("saved map failed native re-open verification");
    }
    reportJson = serializeJson(Json::Object{
        {"schema", "eud-map-edit-report/1"},
        {"ok", true},
        {"operationCount", operations.size()},
        {"effects", std::move(effects)},
        {"inputSha256", expectedHash},
        {"outputSha256", readFileSha256(outputPath)},
        {"extraAssetsDigest", inventoryDigest(afterAssets)}
    });
    return 0;
}

int renderRegion(const char* mapPath, const char* starCraftPath, const std::uint8_t* requestJson,
    std::size_t requestLength, std::vector<std::uint8_t>& rgba, std::uint32_t& width, std::uint32_t& height)
{
    if ( mapPath == nullptr || starCraftPath == nullptr || requestJson == nullptr || requestLength == 0 )
        fail("render received an invalid argument");
    const Json parsed = parseJson(std::string(reinterpret_cast<const char*>(requestJson), requestLength), "render request");
    const auto& request = objectValue(parsed, "render request");
    if ( stringValue(requiredField(request, "schema", "render request"), "render request.schema") != RenderSchema )
        fail("unsupported render schema");
    const std::string mode = stringValue(requiredField(request, "mode", "render request"), "render request.mode");
    const auto assets = loadAssets(starCraftPath);
    if ( mode == "thumbnail" )
    {
        const std::size_t tileset = checkedSize(request, "tileset", "render request");
        if ( tileset >= Sc::Terrain::NumTilesets ) fail("render thumbnail tileset is out of range");
        renderThumbnail(request, static_cast<Sc::Terrain::Tileset>(tileset), *assets, rgba, width, height);
        return 0;
    }
    if ( mode != "region" ) fail("unsupported render mode");
    MapFile map(mapPath);
    if ( map.empty() ) fail("cannot load render map");
    allowedFields(request, {"schema", "mode", "x", "y", "width", "height", "scale", "layers"}, "render request");
    const std::size_t cropX = checkedSize(request, "x", "render request");
    const std::size_t cropY = checkedSize(request, "y", "render request");
    const std::size_t cropWidth = checkedSize(request, "width", "render request", true);
    const std::size_t cropHeight = checkedSize(request, "height", "render request", true);
    const std::size_t scale = checkedSize(request, "scale", "render request", true);
    if ( scale != 1 && scale != 2 && scale != 4 && scale != 8 ) fail("render scale must be 1, 2, 4, or 8");
    if ( cropX + cropWidth > map.getTileWidth() || cropY + cropHeight > map.getTileHeight() ) fail("render crop is outside map bounds");
    const auto& layerValues = arrayValue(requiredField(request, "layers", "render request"), "render request.layers");
    std::set<std::string> layers;
    for ( const auto& value : layerValues ) layers.insert(stringValue(value, "render request.layers[]"));
    const std::set<std::string> allowed{"terrain", "doodads", "sprites", "units", "buildings", "locations"};
    for ( const auto& layer : layers ) if ( allowed.find(layer) == allowed.end() ) fail("unsupported render layer: " + layer);

    const std::size_t outputWidth = cropWidth * 32 / scale;
    const std::size_t outputHeight = cropHeight * 32 / scale;
    if ( outputWidth == 0 || outputHeight == 0 || outputWidth > 8192 || outputHeight > 8192 ) fail("render output dimensions are out of range");
    width = static_cast<std::uint32_t>(outputWidth);
    height = static_cast<std::uint32_t>(outputHeight);
    rgba.assign(outputWidth * outputHeight * 4, 0);
    for ( std::size_t alpha = 3; alpha < rgba.size(); alpha += 4 )
        rgba[alpha] = 255;
    const auto& tiles = assets->terrain.get(map.getTileset());
    const std::int64_t cropLeft = static_cast<std::int64_t>(cropX * 32);
    const std::int64_t cropTop = static_cast<std::int64_t>(cropY * 32);
    const bool terrainVisible = layers.find("terrain") != layers.end() || layers.empty();
    if ( terrainVisible && map.getTileset() == Sc::Terrain::Tileset::SpacePlatform )
    {
        if ( assets->starLayers.empty() )
            fail("cannot load StarCraft Space Platform star parallax");
        drawStarParallax(rgba, outputWidth, outputHeight, cropLeft, cropTop, scale, *assets, tiles);
    }
    if ( terrainVisible )
    {
        for ( std::size_t outY = 0; outY < outputHeight; ++outY )
        {
            const std::size_t mapY = cropY * 32 + outY * scale;
            for ( std::size_t outX = 0; outX < outputWidth; ++outX )
            {
                const std::size_t mapX = cropX * 32 + outX * scale;
                const std::uint16_t tile = map.getTile(mapX / 32, mapY / 32);
                putTerrainPixel(rgba, outputWidth, outX, outY, tile, mapX % 32, mapY % 32, tiles);
            }
        }
    }
    if ( layers.find("sprites") != layers.end() || layers.find("doodads") != layers.end() )
    {
        using SpriteKey = std::tuple<std::uint16_t, std::uint16_t, std::uint16_t, std::uint8_t, std::uint16_t>;
        const auto keyOf = [](const Chk::Sprite& sprite) {
            return SpriteKey{static_cast<std::uint16_t>(sprite.type), sprite.xc, sprite.yc,
                sprite.owner, sprite.flags};
        };
        std::map<SpriteKey, std::size_t> doodadOverlays;
        for ( std::size_t index = 0; index < map.numDoodads(); ++index )
        {
            const auto overlay = doodadOverlay(map, map.getDoodad(index), *assets, "render doodad overlay");
            if ( overlay.has_value() ) ++doodadOverlays[keyOf(*overlay)];
        }
        std::vector<bool> isDoodadOverlay(map.numSprites(), false);
        for ( std::size_t index = 0; index < map.numSprites(); ++index )
        {
            const auto key = keyOf(map.getSprite(index));
            auto found = doodadOverlays.find(key);
            if ( found != doodadOverlays.end() && found->second > 0 )
            {
                isDoodadOverlay[index] = true;
                --found->second;
            }
        }
        for ( bool drawDoodads : {true, false} )
        {
            if ( drawDoodads && layers.find("doodads") == layers.end() ) continue;
            if ( !drawDoodads && layers.find("sprites") == layers.end() ) continue;
            for ( std::size_t index = 0; index < map.numSprites(); ++index )
            {
                if ( isDoodadOverlay[index] != drawDoodads ) continue;
                const auto& sprite = map.getSprite(index);
                drawGrp(rgba, outputWidth, outputHeight, cropLeft, cropTop, scale,
                    static_cast<std::uint16_t>(sprite.type), sprite.owner, 0,
                    sprite.xc, sprite.yc, *assets, tiles);
            }
        }
    }
    if ( layers.find("units") != layers.end() || layers.find("buildings") != layers.end() )
    {
        for ( std::size_t index = 0; index < map.numUnits(); ++index )
        {
            const auto& unit = map.getUnit(index);
            if ( static_cast<std::size_t>(unit.type) >= Sc::Unit::TotalTypes ) continue;
            const auto& unitData = assets->units.getUnit(unit.type);
            const bool building = (unitData.starEditGroupFlags & 0x10) != 0;
            if ( building && layers.find("buildings") == layers.end() ) continue;
            if ( !building && layers.find("units") == layers.end() ) continue;
            const std::uint16_t sprite = unitSpriteId(*assets, static_cast<std::uint16_t>(unit.type));
            if ( sprite == std::numeric_limits<std::uint16_t>::max() ) continue;
            drawGrp(rgba, outputWidth, outputHeight, cropLeft, cropTop, scale, sprite, unit.owner,
                unitData.unitDirection, unit.xc, unit.yc, *assets, tiles);
        }
    }
    return 0;
}

int catalogQuery(const char* starCraftPath, const std::uint8_t* requestJson, std::size_t requestLength, std::string& resultJson)
{
    if ( starCraftPath == nullptr || requestJson == nullptr || requestLength == 0 ) fail("catalog received an invalid argument");
    const auto assets = loadAssets(starCraftPath);
    const Json parsed = parseJson(std::string(reinterpret_cast<const char*>(requestJson), requestLength), "catalog request");
    const auto& request = objectValue(parsed, "catalog request");
    allowedFields(request, {"schema", "kind", "tileset", "offset", "limit", "query", "filter"}, "catalog request");
    if ( stringValue(requiredField(request, "schema", "catalog request"), "catalog request.schema") != CatalogSchema )
        fail("unsupported catalog schema");
    const std::string kind = stringValue(requiredField(request, "kind", "catalog request"), "catalog request.kind");
    const auto tileset = Sc::Terrain::Tileset(checkedSize(request, "tileset", "catalog request") % 8);
    const std::size_t offset = optionalField(request, "offset") == nullptr ? 0 : sizeValue(*optionalField(request, "offset"), "catalog request.offset");
    const std::size_t limit = optionalField(request, "limit") == nullptr ? 100 : sizeValue(*optionalField(request, "limit"), "catalog request.limit", true);
    if ( limit > 512 ) fail("catalog limit exceeds 512");
    const std::string query = optionalField(request, "query") == nullptr ? std::string() : lowerAscii(stringValue(*optionalField(request, "query"), "catalog request.query"));
    const CatalogFilter filter = parseCatalogFilter(request, kind);
    Json::Array page;
    std::size_t totalMatches = 0;
    auto appendEntry = [&](Json entry) {
        const auto& entryObject = objectValue(entry, "catalog entry");
        const std::string name = lowerAscii(stringValue(requiredField(entryObject, "name", "catalog entry"), "catalog entry.name"));
        if ( !query.empty() && name.find(query) == std::string::npos ) return;
        if ( !catalogFilterMatches(entryObject, filter) ) return;
        if ( totalMatches >= offset && page.size() < limit ) page.push_back(std::move(entry));
        ++totalMatches;
    };
    const auto& tiles = assets->terrain.get(tileset);
    if ( kind == "tiles" )
    {
        const std::size_t total = std::min<std::size_t>(Sc::Terrain::Cv5Dat::MaxTileGroups, tiles.tileGroups.size()) * 16;
        for ( std::size_t id = 0; id < total; ++id ) appendEntry(tileEntry(tiles, tileset, static_cast<std::uint16_t>(id)));
    }
    else if ( kind == "brushes" )
    {
        for ( const auto& brush : assets->isom.get(tileset).brushes )
        {
            std::size_t previewTile = 0;
            bool foundPreview = false;
            const std::size_t groupCount = std::min<std::size_t>(
                Sc::Terrain::Cv5Dat::MaxTileGroups,
                tiles.tileGroups.size());
            for ( std::size_t group = 0; group < groupCount && !foundPreview; ++group )
            {
                if ( tiles.tileGroups[group].terrainType != brush.index )
                    continue;
                for ( std::size_t variant = 0; variant < 16; ++variant )
                {
                    const auto tile = static_cast<std::uint16_t>(group * 16 + variant);
                    if ( tileGraphicsValid(tiles, tile) )
                    {
                        previewTile = tile;
                        foundPreview = true;
                        break;
                    }
                }
            }
            Json::Object entry{{"id", static_cast<std::size_t>(brush.index)}, {"name", std::string(brush.name)},
                {"terrainType", static_cast<std::size_t>(brush.index)}, {"tileset", tilesetName(tileset)},
                {"previewTile", previewTile}, {"graphicsValid", foundPreview},
                {"fingerprint", sha256Bytes(&brush.index, sizeof(brush.index)) + ":" + std::string(brush.name)}};
            if ( foundPreview )
            {
                const Json previewJson = tileEntry(tiles, tileset, static_cast<std::uint16_t>(previewTile));
                const auto& preview = objectValue(previewJson, "brush preview metadata");
                for ( const char* key : {"buildability", "groundHeight", "megaTile", "walkability",
                         "ramp", "blocksView", "highMinitiles", "midMinitiles"} )
                {
                    const auto found = preview.find(key);
                    if ( found != preview.end() ) entry.insert_or_assign(key, found->second);
                }
            }
            appendEntry(std::move(entry));
        }
    }
    else if ( kind == "units" || kind == "buildings" )
    {
        for ( std::size_t id = 0; id < Sc::Unit::TotalTypes; ++id )
        {
            const auto& unit = assets->units.getUnit(Sc::Unit::Type(id));
            const bool building = (unit.starEditGroupFlags & 0x10) != 0;
            if ( (kind == "buildings") != building ) continue;
            const std::string name = id < Sc::Unit::defaultDisplayNames.size() ? Sc::Unit::defaultDisplayNames[id] : "Unit " + std::to_string(id);
            appendEntry(Json::Object{{"id", id}, {"name", name}, {"building", building},
                {"placementWidth", static_cast<std::size_t>(unit.starEditPlacementBoxWidth)},
                {"placementHeight", static_cast<std::size_t>(unit.starEditPlacementBoxHeight)},
                {"extentLeft", static_cast<std::size_t>(unit.unitSizeLeft)}, {"extentUp", static_cast<std::size_t>(unit.unitSizeUp)},
                {"extentRight", static_cast<std::size_t>(unit.unitSizeRight)}, {"extentDown", static_cast<std::size_t>(unit.unitSizeDown)},
                {"fingerprint", sha256Bytes(&unit, sizeof(unit))}});
        }
    }
    else if ( kind == "sprites" )
    {
        for ( std::size_t id = 0; id < assets->spriteEntries.size(); ++id )
        {
            const auto& sprite = assets->sprite(id);
            appendEntry(Json::Object{{"id", id}, {"name", spriteName(*assets, id)},
                {"visible", sprite.isVisible != 0}, {"fingerprint", sha256Bytes(&sprite, sizeof(sprite))}});
        }
    }
    else if ( kind == "doodads" )
    {
        const auto& doodads = assets->doodads(tileset);
        for ( std::size_t id = 0; id < doodads.size(); ++id )
        {
            const auto& doodad = doodads[id];
            bool graphicsValid = doodad.doodadWidth > 0 && doodad.doodadHeight > 0
                && doodad.doodadWidth * doodad.doodadHeight <= 16;
            for ( std::size_t cell = 0; graphicsValid && cell < doodad.doodadWidth * doodad.doodadHeight; ++cell )
            {
                const auto tile = static_cast<std::uint16_t>((Sc::Terrain::Cv5Dat::MaxTileGroups + id) * 16 + cell);
                graphicsValid = tileGraphicsValid(tiles, tile);
            }
            std::string name;
            if ( !assets->statTxt.getString(doodad.doodadName, name) || name.empty() ) name = "Doodad " + std::to_string(id);
            appendEntry(Json::Object{{"id", id}, {"name", name},
                {"width", static_cast<std::size_t>(doodad.doodadWidth)}, {"height", static_cast<std::size_t>(doodad.doodadHeight)},
                {"buildability", static_cast<std::size_t>(doodad.buildability)}, {"graphicsValid", graphicsValid},
                {"overlay", (doodad.flags & 0x30) != 0}, {"overlayId", static_cast<std::size_t>(doodad.overlayIndex)},
                {"overlayFlags", static_cast<std::size_t>((doodad.flags & 0x10) != 0 ? static_cast<std::uint16_t>(Chk::Sprite::SpriteFlags::DrawAsSprite) : 0)},
                {"fingerprint", sha256Bytes(&doodad, sizeof(doodad))}});
        }
    }
    else
        fail("unsupported catalog kind: " + kind);

    resultJson = serializeJson(Json::Object{{"schema", "eud-map-catalog-result/1"}, {"kind", kind},
        {"tileset", tilesetName(tileset)}, {"total", totalMatches}, {"offset", offset}, {"entries", std::move(page)}});
    return 0;
}

int imageQuantize(
    const char* starCraftPath,
    std::uint16_t tileset,
    const std::uint8_t* rgba,
    std::size_t rgbaLength,
    std::uint16_t width,
    std::uint16_t height,
    const std::uint16_t* beforeTiles,
    std::size_t beforeTileCount,
    std::vector<std::uint8_t>& result)
{
    if ( starCraftPath == nullptr || starCraftPath[0] == '\0'
         || rgba == nullptr || beforeTiles == nullptr
         || width == 0 || height == 0 || width > 256 || height > 256
         || tileset >= Sc::Terrain::NumTilesets )
        fail("image quantizer received an invalid argument");
    const std::size_t cellCount = static_cast<std::size_t>(width) * height;
    if ( cellCount > 65536
         || rgbaLength != cellCount * 4
         || beforeTileCount != cellCount )
        fail("image quantizer buffer dimensions do not match");

    const auto assets = loadAssets(starCraftPath);
    const auto nativeTileset = Sc::Terrain::Tileset(tileset);
    const auto& terrain = assets->terrain.get(nativeTileset);
    const auto palette = imagePalette(starCraftPath, *assets, nativeTileset);
    static constexpr std::array<std::uint8_t, 64> Bayer8{
         0, 48, 12, 60,  3, 51, 15, 63,
        32, 16, 44, 28, 35, 19, 47, 31,
         8, 56,  4, 52, 11, 59,  7, 55,
        40, 24, 36, 20, 43, 27, 39, 23,
         2, 50, 14, 62,  1, 49, 13, 61,
        34, 18, 46, 30, 33, 17, 45, 29,
        10, 58,  6, 54,  9, 57,  5, 53,
        42, 26, 38, 22, 41, 25, 37, 21,
    };
    std::map<std::uint16_t, ImageTileColor> beforeColors;
    const auto beforeColor = [&beforeColors, &terrain](std::uint16_t tile) -> const ImageTileColor& {
        const auto found = beforeColors.find(tile);
        if ( found != beforeColors.end() )
            return found->second;
        const auto color = imageTileColor(terrain, tile);
        if ( !color.has_value() )
            fail("image quantizer candidate terrain contains invalid or transparent-only graphics");
        return beforeColors.emplace(tile, *color).first->second;
    };

    std::vector<std::uint16_t> outputTiles;
    outputTiles.reserve(cellCount);
    std::vector<std::uint8_t> previewRgb;
    previewRgb.reserve(cellCount * 3);
    std::set<std::uint16_t> uniqueTiles;
    std::uint32_t walkabilityChanged = 0;
    std::uint32_t heightChanged = 0;
    for ( std::size_t index = 0; index < cellCount; ++index )
    {
        const auto& background = beforeColor(beforeTiles[index]);
        const std::size_t source = index * 4;
        const std::uint8_t alpha = rgba[source + 3];
        const ImageTileColor* chosen = &background;
        if ( alpha != 0 )
        {
            std::array<std::uint8_t, 3> composite{};
            const std::array<std::uint8_t, 3> backgroundRgb{
                background.red,
                background.green,
                background.blue,
            };
            for ( std::size_t channel = 0; channel < composite.size(); ++channel )
            {
                const std::uint32_t value =
                    static_cast<std::uint32_t>(rgba[source + channel]) * alpha
                    + static_cast<std::uint32_t>(backgroundRgb[channel]) * (255 - alpha);
                const std::int32_t blended = static_cast<std::int32_t>((value + 127) / 255);
                const std::int32_t centered =
                    static_cast<std::int32_t>(Bayer8[(index / width % 8) * 8 + (index % width % 8)]) * 2 - 63;
                const std::int32_t adjusted = blended + centered * 24 / 63;
                composite[channel] = static_cast<std::uint8_t>(
                    std::max<std::int32_t>(0, std::min<std::int32_t>(255, adjusted)));
            }
            std::size_t nearest = std::numeric_limits<std::size_t>::max();
            std::uint32_t nearestDistance = std::numeric_limits<std::uint32_t>::max();
            nearestImageTile(
                *palette,
                palette->root,
                composite,
                nearest,
                nearestDistance);
            if ( nearest == std::numeric_limits<std::size_t>::max() )
                fail("image quantizer could not resolve a palette tile");
            chosen = &palette->entries[nearest];
        }

        outputTiles.push_back(chosen->tile);
        previewRgb.push_back(chosen->red);
        previewRgb.push_back(chosen->green);
        previewRgb.push_back(chosen->blue);
        uniqueTiles.insert(chosen->tile);
        if ( chosen->tile != beforeTiles[index] )
        {
            if ( chosen->walkability != background.walkability )
                ++walkabilityChanged;
            if ( chosen->height != background.height )
                ++heightChanged;
        }
    }

    const std::size_t resultSize = 20 + cellCount * 5;
    result.clear();
    result.reserve(resultSize);
    result.insert(result.end(), {'M', 'I', 'Q', '1'});
    appendU16(result, width);
    appendU16(result, height);
    appendU32(result, static_cast<std::uint32_t>(uniqueTiles.size()));
    appendU32(result, walkabilityChanged);
    appendU32(result, heightChanged);
    for ( const auto tile : outputTiles )
        appendU16(result, tile);
    result.insert(result.end(), previewRgb.begin(), previewRgb.end());
    if ( result.size() != resultSize )
        fail("image quantizer result length invariant failed");
    return 0;
}

int mapDigest(const char* mapPath, std::string& resultJson)
{
    if ( mapPath == nullptr || mapPath[0] == '\0' ) fail("map digest received an invalid path");
    MapFile map(mapPath);
    if ( map.empty() ) fail("cannot load map for digest");
    const AssetInventory assets = inventoryMpq(mapPath);
    resultJson = serializeJson(Json::Object{{"schema", "eud-map-container-digest/1"},
        {"fileSha256", readFileSha256(mapPath)}, {"tileset", tilesetName(map.getTileset())},
        {"width", map.getTileWidth()}, {"height", map.getTileHeight()}, {"extraAssets", inventoryJson(assets)}});
    return 0;
}

} // namespace mapagent
