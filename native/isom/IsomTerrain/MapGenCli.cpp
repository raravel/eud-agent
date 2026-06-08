// isom-poc: grid file -> ISOM-first StarCraft map (.scx) CLI
// Reuses the exact API patterns from IsomTests.cpp (newMap/placeTerrain),
// kept in an anonymous namespace to avoid ODR collisions with IsomTests.cpp.
#include "IsomApi.h"
#include "../CrossCutLib/Logger.h"
#include "../MappingCoreLib/MappingCore.h"
#include <algorithm>
#include <array>
#include <cctype>
#include <chrono>
#include <fstream>
#include <iostream>
#include <map>
#include <set>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

extern Logger logger;          // defined in IsomTests.cpp
extern Sc::Terrain_ terrainDat; // defined in IsomTests.cpp

namespace
{
    const std::map<std::string, Sc::Terrain::Tileset> tilesetNames {
        { "badlands",      Sc::Terrain::Tileset::Badlands },
        { "spaceplatform", Sc::Terrain::Tileset::SpacePlatform },
        { "platform",      Sc::Terrain::Tileset::SpacePlatform },
        { "installation",  Sc::Terrain::Tileset::Installation },
        { "ashworld",      Sc::Terrain::Tileset::Ashworld },
        { "jungle",        Sc::Terrain::Tileset::Jungle },
        { "desert",        Sc::Terrain::Tileset::Desert },
        { "arctic",        Sc::Terrain::Tileset::Arctic },
        { "ice",           Sc::Terrain::Tileset::Arctic },
        { "twilight",      Sc::Terrain::Tileset::Twilight }
    };

    std::string normalizeName(const std::string & name)
    {
        std::string result {};
        for ( char c : name )
        {
            if ( !std::isspace(static_cast<unsigned char>(c)) && c != '_' && c != '-' )
                result.push_back(static_cast<char>(std::tolower(static_cast<unsigned char>(c))));
        }
        return result;
    }

    ScMap copyToScMap(const MapFile & src)
    {
        ScMap dest {};
        dest.tileWidth = uint16_t(src.getTileWidth());
        dest.tileHeight = uint16_t(src.getTileHeight());
        dest.tileset = src.getTileset();
        dest.isomRects.assign(src.isomRects.size(), {});
        std::memcpy(&dest.isomRects[0], &src.isomRects[0], src.isomRects.size()*sizeof(Chk::IsomRect)); // ISOM
        dest.editorTiles = src.editorTiles; // TILE
        dest.tiles = src.tiles; // MTXM
        return dest;
    }

    void copyFromScMap(MapFile & dest, const ScMap & src)
    {
        dest.dimensions.tileWidth = src.tileWidth;
        dest.dimensions.tileHeight = src.tileHeight;
        dest.tileset = src.tileset;
        dest.isomRects.assign(src.isomRects.size(), {});
        std::memcpy(&dest.isomRects[0], &src.isomRects[0], src.isomRects.size()*sizeof(Chk::IsomRect)); // ISOM
        dest.editorTiles = src.editorTiles; // TILE
        dest.tiles = src.tiles; // MTXM
    }

    // Same as IsomTests.cpp newMap(): fill the whole map with one terrain type
    std::unique_ptr<MapFile> newFilledMap(Sc::Terrain::Tileset tileset, uint16_t width, uint16_t height, size_t terrainType)
    {
        auto mapFile = std::make_unique<MapFile>(tileset, width, height);
        ScMap scMap = copyToScMap(*mapFile);

        Chk::IsomCache isomCache(tileset, width, height, terrainDat.get(tileset));

        uint16_t isomValue = uint16_t((isomCache.getTerrainTypeIsomValue(terrainType) << 4) | Chk::IsomRect::EditorFlag::Modified);
        scMap.isomRects.assign(scMap.getIsomWidth()*scMap.getIsomHeight(), Chk::IsomRect{isomValue, isomValue, isomValue, isomValue});

        isomCache.setAllChanged();
        scMap.updateTilesFromIsom(isomCache);

        copyFromScMap(*mapFile, scMap);
        return mapFile;
    }

    size_t findTerrainType(const Sc::Terrain_::Tiles & tilesetData, const std::string & nameOrId)
    {
        if ( !nameOrId.empty() && std::all_of(nameOrId.begin(), nameOrId.end(), [](char c){ return std::isdigit(static_cast<unsigned char>(c)); }) )
            return size_t(std::stoul(nameOrId));

        const std::string normalized = normalizeName(nameOrId);
        for ( const auto & brush : tilesetData.brushes )
        {
            if ( normalizeName(std::string(brush.name)) == normalized )
                return size_t(brush.index);
        }
        return 0; // 0 = invalid terrain type
    }

    // CLI는 전자동 도구이므로 GUI 폴백 없는 데이터 로더를 사용한다. 브라우저를 전부
    // nullptr로 두면 실패 시 대화상자 대신 빈 결과로 떨어지고, 동시 실행으로 CASC
    // 오픈이 일시 실패하는 경우는 짧은 재시도로 흡수한다. (no dialogs)
    std::vector<ArchiveFilePtr> openScDataFilesSilent(const std::string & starcraftPath)
    {
        std::vector<Sc::DataFile::Descriptor> dataFiles {};
        for ( const auto & d : Sc::DataFile::getDefaultDataFiles() )
        {
            dataFiles.emplace_back(d.getPriority(), d.isCasc(), d.isOptionalIfCascFound(),
                d.getFileName(), d.getExpectedFilePath(), nullptr, d.isExpectedInScDirectory());
        }
        Sc::DataFile::Browser dataFileBrowser {};
        for ( int attempt = 0; attempt < 3; ++attempt )
        {
            if ( attempt > 0 )
                std::this_thread::sleep_for(std::chrono::milliseconds(500));
            std::vector<ArchiveFilePtr> orderedSourceFiles = dataFileBrowser.openScDataFiles(dataFiles, starcraftPath, nullptr);
            if ( !orderedSourceFiles.empty() )
                return orderedSourceFiles;
        }
        return {};
    }

    bool loadTerrainDatSilent(const std::string & starcraftPath)
    {
        const std::vector<ArchiveFilePtr> orderedSourceFiles = openScDataFilesSilent(starcraftPath);
        return !orderedSourceFiles.empty() && terrainDat.load(orderedSourceFiles);
    }

    int listBrushes(const std::string & tilesetName, const std::string & starcraftPath)
    {
        auto tilesetIt = tilesetNames.find(normalizeName(tilesetName));
        if ( tilesetIt == tilesetNames.end() )
        {
            std::cerr << "Unknown tileset: " << tilesetName << std::endl;
            return 1;
        }
        if ( !loadTerrainDatSilent(starcraftPath) )
        {
            std::cerr << "Failed to load terrain data from: " << starcraftPath << std::endl;
            return 1;
        }
        const auto & tilesetData = terrainDat.get(tilesetIt->second);
        std::cout << "Brushes for tileset \"" << tilesetIt->first << "\":" << std::endl;
        for ( const auto & brush : tilesetData.brushes )
            std::cout << "  " << brush.index << " : " << brush.name << std::endl;
        return 0;
    }

    struct StampOp
    {
        std::string file;
        size_t x = 0;
        size_t y = 0;
    };

    struct CarveOp // 사각 영역을 스탬프 이전의 바탕(등각) 지형으로 복원 = 길 뚫기
    {
        size_t x = 0;
        size_t y = 0;
        size_t w = 0;
        size_t h = 0;
    };

    struct UnitOp // unit <player> <tileX> <tileY> <count> <unit name|id>
    {
        size_t owner = 0;
        size_t x = 0;
        size_t y = 0;
        size_t count = 1;
        std::string name;
    };

    struct LocationOp // location <tileX> <tileY> <w> <h> <이름>
    {
        size_t x = 0;
        size_t y = 0;
        size_t w = 0;
        size_t h = 0;
        std::string name;
    };

    // "P1"~"P12", "1"~"12", "neutral" → 0-based owner
    bool parsePlayer(const std::string & token, size_t & owner)
    {
        std::string t = normalizeName(token);
        if ( t == "neutral" )
        {
            owner = 11;
            return true;
        }
        if ( !t.empty() && t[0] == 'p' )
            t = t.substr(1);
        if ( t.empty() || !std::all_of(t.begin(), t.end(), [](char c){ return std::isdigit(static_cast<unsigned char>(c)); }) )
            return false;
        size_t n = std::stoul(t);
        if ( n < 1 || n > 12 )
            return false;
        owner = n - 1;
        return true;
    }

    int findUnitType(const std::string & nameOrId)
    {
        if ( !nameOrId.empty() && std::all_of(nameOrId.begin(), nameOrId.end(), [](char c){ return std::isdigit(static_cast<unsigned char>(c)); }) )
        {
            size_t id = std::stoul(nameOrId);
            return id < Sc::Unit::defaultDisplayNames.size() ? int(id) : -1;
        }
        const std::string normalized = normalizeName(nameOrId);
        for ( size_t i = 0; i < Sc::Unit::defaultDisplayNames.size(); ++i )
        {
            if ( normalizeName(Sc::Unit::defaultDisplayNames[i]) == normalized )
                return int(i);
        }
        return -1;
    }

    bool parseUnitLine(const std::string & rest, UnitOp & op)
    {
        std::istringstream ss(rest);
        std::string player {};
        ss >> player >> op.x >> op.y >> op.count;
        std::getline(ss >> std::ws, op.name);
        return parsePlayer(player, op.owner) && op.count >= 1 && !op.name.empty();
    }

    struct GridSpec
    {
        Sc::Terrain::Tileset tileset = Sc::Terrain::Tileset::Jungle;
        std::string tilesetName = "jungle";
        uint16_t width = 64;
        uint16_t height = 64;
        std::string fillName = "";
        size_t brushExtent = 1;
        std::map<char, std::string> legend {};
        std::vector<std::string> gridRows {};
        std::vector<StampOp> stamps {};
        std::vector<CarveOp> carves {};
        std::vector<UnitOp> units {};
        std::vector<UnitOp> starts {};
        std::vector<LocationOp> locationOps {};
        bool noRepair = false; // grid header "--no-repair" disables the seam repair pass
    };

    // .stamp 파일: MTXM 사각 영역의 타일 원본 (extended terrain 조각)
    struct Stamp
    {
        std::string tilesetName;
        size_t width = 0;
        size_t height = 0;
        std::vector<uint16_t> tiles {};
    };

    bool loadStamp(const std::string & path, Stamp & stamp)
    {
        std::ifstream in(path);
        if ( !in )
        {
            std::cerr << "Cannot open stamp: " << path << std::endl;
            return false;
        }
        std::string line {};
        bool inTiles = false;
        while ( std::getline(in, line) )
        {
            if ( !line.empty() && line.back() == '\r' )
                line.pop_back();
            if ( line.empty() || line[0] == '#' )
                continue;
            if ( inTiles )
            {
                std::istringstream ss(line);
                std::string tok {};
                while ( ss >> tok )
                    stamp.tiles.push_back(uint16_t(std::stoul(tok, nullptr, 16)));
                continue;
            }
            std::istringstream ss(line);
            std::string keyword {};
            ss >> keyword;
            if ( keyword == "tileset" )
                ss >> stamp.tilesetName;
            else if ( keyword == "size" )
                ss >> stamp.width >> stamp.height;
            else if ( keyword == "tiles" )
                inTiles = true;
        }
        return stamp.width > 0 && stamp.tiles.size() == stamp.width * stamp.height;
    }

    bool parseGridFile(const std::string & path, GridSpec & spec)
    {
        std::ifstream in(path);
        if ( !in )
        {
            std::cerr << "Cannot open grid file: " << path << std::endl;
            return false;
        }
        std::string line {};
        bool inGrid = false;
        while ( std::getline(in, line) )
        {
            if ( !line.empty() && line.back() == '\r' )
                line.pop_back();

            if ( inGrid )
            {
                if ( line.rfind("stamp ", 0) == 0 ) // grid 섹션 뒤에 오는 stamp 지시문 허용
                {
                    std::istringstream ss(line.substr(6));
                    StampOp op {};
                    ss >> op.file >> op.x >> op.y;
                    if ( !op.file.empty() )
                        spec.stamps.push_back(op);
                }
                else if ( line.rfind("carve ", 0) == 0 )
                {
                    std::istringstream ss(line.substr(6));
                    CarveOp op {};
                    ss >> op.x >> op.y >> op.w >> op.h;
                    if ( op.w > 0 && op.h > 0 )
                        spec.carves.push_back(op);
                }
                else if ( line.rfind("unit ", 0) == 0 )
                {
                    UnitOp op {};
                    if ( !parseUnitLine(line.substr(5), op) )
                    {
                        std::cerr << "Bad unit line: " << line << std::endl;
                        return false;
                    }
                    spec.units.push_back(op);
                }
                else if ( line.rfind("start ", 0) == 0 )
                {
                    std::istringstream ss(line.substr(6));
                    std::string player {};
                    UnitOp op {};
                    ss >> player >> op.x >> op.y;
                    if ( !parsePlayer(player, op.owner) )
                    {
                        std::cerr << "Bad start line: " << line << std::endl;
                        return false;
                    }
                    spec.starts.push_back(op);
                }
                else if ( line.rfind("location ", 0) == 0 )
                {
                    std::istringstream ss(line.substr(9));
                    LocationOp op {};
                    ss >> op.x >> op.y >> op.w >> op.h;
                    std::getline(ss >> std::ws, op.name);
                    if ( op.w == 0 || op.h == 0 || op.name.empty() )
                    {
                        std::cerr << "Bad location line: " << line << std::endl;
                        return false;
                    }
                    spec.locationOps.push_back(op);
                }
                else if ( !line.empty() )
                    spec.gridRows.push_back(line);
                continue;
            }
            if ( line.empty() || line[0] == '#' )
                continue;

            if ( line.rfind("--no-repair", 0) == 0 ) // header flag: skip the seam repair pass
            {
                spec.noRepair = true;
                continue;
            }

            std::istringstream ss(line);
            std::string keyword {};
            ss >> keyword;
            if ( keyword == "tileset" )
            {
                ss >> spec.tilesetName;
                auto it = tilesetNames.find(normalizeName(spec.tilesetName));
                if ( it == tilesetNames.end() )
                {
                    std::cerr << "Unknown tileset: " << spec.tilesetName << std::endl;
                    return false;
                }
                spec.tileset = it->second;
            }
            else if ( keyword == "size" )
                ss >> spec.width >> spec.height;
            else if ( keyword == "fill" )
                std::getline(ss >> std::ws, spec.fillName);
            else if ( keyword == "brush" )
                ss >> spec.brushExtent;
            else if ( keyword == "legend" )
            {
                char symbol = '\0';
                std::string name {};
                ss >> symbol;
                std::getline(ss >> std::ws, name);
                if ( symbol != '\0' && !name.empty() )
                    spec.legend[symbol] = name;
            }
            else if ( keyword == "stamp" )
            {
                StampOp op {};
                ss >> op.file >> op.x >> op.y;
                if ( !op.file.empty() )
                    spec.stamps.push_back(op);
            }
            else if ( keyword == "grid" )
                inGrid = true;
            else if ( keyword == "unit" )
            {
                UnitOp op {};
                std::string rest {};
                std::getline(ss >> std::ws, rest);
                if ( !parseUnitLine(rest, op) )
                {
                    std::cerr << "Bad unit line: " << line << std::endl;
                    return false;
                }
                spec.units.push_back(op);
            }
            else if ( keyword == "start" )
            {
                std::string player {};
                UnitOp op {};
                ss >> player >> op.x >> op.y;
                if ( !parsePlayer(player, op.owner) )
                {
                    std::cerr << "Bad start line: " << line << std::endl;
                    return false;
                }
                spec.starts.push_back(op);
            }
            else if ( keyword == "location" )
            {
                LocationOp op {};
                ss >> op.x >> op.y >> op.w >> op.h;
                std::getline(ss >> std::ws, op.name);
                if ( op.w == 0 || op.h == 0 || op.name.empty() )
                {
                    std::cerr << "Bad location line: " << line << std::endl;
                    return false;
                }
                spec.locationOps.push_back(op);
            }
            else
            {
                std::cerr << "Unknown keyword: " << keyword << std::endl;
                return false;
            }
        }
        if ( spec.gridRows.empty() )
        {
            std::cerr << "Grid file has no grid section" << std::endl;
            return false;
        }
        if ( spec.fillName.empty() )
        {
            std::cerr << "Grid file has no fill terrain" << std::endl;
            return false;
        }
        return true;
    }

    // Count walkable minitiles for a tile value, using VF4 tileFlags (Sc::Terrain::Tiles).
    // Returns 0 if the tile's group/megaTile is out of range (treated as non-walkable).
    size_t walkableCount(uint16_t tileValue, const Sc::Terrain::Tiles & vf4Data)
    {
        const size_t group = size_t(tileValue) / 16;
        if ( group >= vf4Data.tileGroups.size() )
            return 0;
        const uint16_t megaIdx = vf4Data.tileGroups[group].megaTileIndex[tileValue % 16];
        if ( size_t(megaIdx) >= vf4Data.tileFlags.size() )
            return 0;
        size_t count = 0;
        for ( size_t r = 0; r < 4; ++r )
            for ( size_t c = 0; c < 4; ++c )
                if ( vf4Data.tileFlags[megaIdx].miniTileFlags[r][c].isWalkable() )
                    ++count;
        return count;
    }

    // How well candidate-side link "a" matches neighbor-side link "b" for one direction.
    // 1 = matching link values, 0 = no match. Out-of-map neighbor is handled by the caller.
    int edgeMatch(Sc::Isom::Link a, Sc::Isom::Link b)
    {
        return ( a == b && a != Sc::Isom::Link::None ) ? 1 : 0;
    }

    // Repair seam tiles in place: for each target tile, try every candidate (group,member) of the
    // same terrainType (and same groundHeight), pick the one whose directional links best match the
    // 4 neighbors, with a bonus for matching the neighborhood majority terrainType. Candidates that
    // reduce the walkable minitile count vs the original are rejected (traversability preservation).
    // Writes both MTXM (tiles) and TILE (editorTiles). Returns number of tiles changed.
    // Deterministic: the original tile is always a candidate and wins ties (idempotent).
    size_t repairTiles(MapFile & mapFile,
                       const Sc::Terrain_::Tiles & isomData,
                       const Sc::Terrain::Tiles & vf4Data,
                       const std::vector<size_t> & targets,
                       size_t maxPasses = 4)
    {
        const size_t w = mapFile.getTileWidth();
        const size_t h = mapFile.getTileHeight();
        if ( w == 0 || h == 0 || targets.empty() )
            return 0;
        const size_t groupCount = isomData.tileGroups.size();

        // Candidate pool: terrainType -> groups that carry it. Build once per map.
        std::map<uint16_t, std::vector<uint16_t>> groupsByType {};
        for ( uint16_t g = 0; g < groupCount; ++g )
            groupsByType[isomData.tileGroups[g].terrainType].push_back(g);

        auto typeOf = [&](uint16_t v) -> uint16_t {
            const size_t g = size_t(v) / 16;
            return g < groupCount ? isomData.tileGroups[g].terrainType : uint16_t(0);
        };

        size_t totalChanged = 0;
        for ( size_t pass = 0; pass < maxPasses; ++pass )
        {
            size_t changedThisPass = 0;
            for ( size_t idx : targets )
            {
                const size_t tx = idx % w, ty = idx / w;
                const uint16_t orig = mapFile.tiles[idx];
                const size_t origGroup = size_t(orig) / 16;
                if ( origGroup >= groupCount )
                    continue; // unknown group (custom tile): leave alone
                const uint16_t origType = isomData.tileGroups[origGroup].terrainType;
                const uint8_t origHeight = isomData.tileGroups[origGroup].groundHeight;
                const size_t origWalkable = walkableCount(orig, vf4Data);

                // Neighbor tile values and their facing links (left/top/right/bottom of THIS tile
                // matches the opposite side of the neighbor). -1 = out of map (neutral).
                struct Side_ { bool inMap; Sc::Isom::Link neighborLink; };
                auto neighborLink = [&](int nx, int ny, int side) -> Side_ {
                    if ( nx < 0 || ny < 0 || size_t(nx) >= w || size_t(ny) >= h )
                        return { false, Sc::Isom::Link::None };
                    const uint16_t nv = mapFile.tiles[size_t(ny) * w + size_t(nx)];
                    const size_t ng = size_t(nv) / 16;
                    if ( ng >= groupCount )
                        return { false, Sc::Isom::Link::None };
                    const auto & nl = isomData.tileGroups[ng].links;
                    // side: 0=left neighbor (use its right), 1=top (its bottom), 2=right (its left), 3=bottom (its top)
                    switch ( side )
                    {
                        case 0: return { true, nl.right };
                        case 1: return { true, nl.bottom };
                        case 2: return { true, nl.left };
                        default: return { true, nl.top };
                    }
                };
                const Side_ nL = neighborLink(int(tx) - 1, int(ty),     0);
                const Side_ nT = neighborLink(int(tx),     int(ty) - 1, 1);
                const Side_ nR = neighborLink(int(tx) + 1, int(ty),     2);
                const Side_ nB = neighborLink(int(tx),     int(ty) + 1, 3);

                // Neighborhood majority terrainType (8-neighborhood), for the terrain bonus.
                std::map<uint16_t, size_t> typeVotes {};
                for ( int dy = -1; dy <= 1; ++dy )
                    for ( int dx = -1; dx <= 1; ++dx )
                    {
                        if ( dx == 0 && dy == 0 ) continue;
                        const int nx = int(tx) + dx, ny = int(ty) + dy;
                        if ( nx < 0 || ny < 0 || size_t(nx) >= w || size_t(ny) >= h ) continue;
                        ++typeVotes[typeOf(mapFile.tiles[size_t(ny) * w + size_t(nx)])];
                    }
                uint16_t majorityType = origType; size_t majorityVotes = 0;
                for ( const auto & [t, c] : typeVotes )
                    if ( c > majorityVotes ) { majorityType = t; majorityVotes = c; }

                auto scoreFor = [&](uint16_t v) -> int {
                    const auto & links = isomData.tileGroups[size_t(v) / 16].links;
                    int s = 0;
                    if ( nL.inMap ) s += edgeMatch(links.left, nL.neighborLink);
                    if ( nT.inMap ) s += edgeMatch(links.top, nT.neighborLink);
                    if ( nR.inMap ) s += edgeMatch(links.right, nR.neighborLink);
                    if ( nB.inMap ) s += edgeMatch(links.bottom, nB.neighborLink);
                    if ( typeOf(v) == majorityType ) s += 1; // terrain bonus
                    return s;
                };

                // Original tile is always in the running so ties keep it (idempotent).
                uint16_t bestTile = orig;
                int bestScore = scoreFor(orig);
                bool bestIsMajority = ( origType == majorityType );

                auto poolIt = groupsByType.find(origType);
                if ( poolIt != groupsByType.end() )
                {
                    for ( uint16_t g : poolIt->second )
                    {
                        const auto & tg = isomData.tileGroups[g];
                        if ( tg.groundHeight != origHeight )
                            continue; // no elevation jumps
                        // Prefer preserving the original member; fall back to ascending member.
                        for ( uint16_t pref = 0; pref < 16; ++pref )
                        {
                            const uint16_t member = ( pref == 0 ) ? uint16_t(orig % 16) : uint16_t(pref - 1);
                            if ( pref != 0 && member == uint16_t(orig % 16) )
                                continue; // already tried as the preferred member
                            const uint16_t cand = uint16_t(g * 16 + member);
                            if ( cand == orig )
                                continue;
                            if ( walkableCount(cand, vf4Data) < origWalkable )
                                continue; // traversability must not shrink
                            const int s = scoreFor(cand);
                            const bool candMajority = ( typeOf(cand) == majorityType );
                            // Tie-break: higher score, then majority-type, then group asc, then member asc.
                            bool better = false;
                            if ( s > bestScore ) better = true;
                            else if ( s == bestScore )
                            {
                                if ( candMajority && !bestIsMajority ) better = true;
                                else if ( candMajority == bestIsMajority )
                                {
                                    const size_t bg = size_t(bestTile) / 16, bm = size_t(bestTile) % 16;
                                    if ( g < bg || ( g == bg && member < bm ) )
                                    {
                                        // Never let a same-type-but-not-original tile beat the original on a pure tie.
                                        if ( bestTile != orig )
                                            better = true;
                                    }
                                }
                            }
                            if ( better )
                            {
                                bestTile = cand;
                                bestScore = s;
                                bestIsMajority = candMajority;
                            }
                        }
                    }
                }

                if ( bestTile != orig )
                {
                    mapFile.tiles[idx] = bestTile;        // MTXM
                    mapFile.editorTiles[idx] = bestTile;  // TILE
                    ++changedThisPass;
                }
            }
            totalChanged += changedThisPass;
            if ( changedThisPass == 0 )
                break; // converged
        }
        return totalChanged;
    }

    // Build the perimeter ring (1 tile) around a set of stamp rects, clipped to the map and
    // excluding tiles inside any carve rect (carve results must be respected). Returns unique
    // tile indices. rects/carves are (x,y,w,h) in tile units.
    std::vector<size_t> stampRingTargets(size_t mapW, size_t mapH,
                                         const std::vector<std::array<size_t,4>> & rects,
                                         const std::vector<std::array<size_t,4>> & carves)
    {
        auto inCarve = [&](size_t x, size_t y) -> bool {
            for ( const auto & c : carves )
                if ( x >= c[0] && x < c[0] + c[2] && y >= c[1] && y < c[1] + c[3] )
                    return true;
            return false;
        };
        std::set<size_t> ring {};
        for ( const auto & r : rects )
        {
            const long rx = long(r[0]), ry = long(r[1]), rw = long(r[2]), rh = long(r[3]);
            for ( long x = rx - 1; x <= rx + rw; ++x )
            {
                for ( long y = ry - 1; y <= ry + rh; ++y )
                {
                    // Ring = the 1-tile border just outside the rect (skip the rect interior).
                    const bool onBorder = ( x == rx - 1 || x == rx + rw || y == ry - 1 || y == ry + rh );
                    if ( !onBorder )
                        continue;
                    if ( x < 0 || y < 0 || size_t(x) >= mapW || size_t(y) >= mapH )
                        continue;
                    if ( inCarve(size_t(x), size_t(y)) )
                        continue;
                    ring.insert(size_t(y) * mapW + size_t(x));
                }
            }
        }
        return std::vector<size_t>(ring.begin(), ring.end());
    }

    // Load the VF4 tileset data (Sc::Terrain::Tiles) for walkability checks, mirroring walkMap().
    bool loadVf4Tiles(Sc::Terrain::Tileset tileset, const std::string & starcraftPath, Sc::Terrain::Tiles & out)
    {
        const std::vector<ArchiveFilePtr> orderedSourceFiles = openScDataFilesSilent(starcraftPath);
        if ( orderedSourceFiles.empty() )
        {
            std::cerr << "Failed to open StarCraft data files at: " << starcraftPath << std::endl;
            return false;
        }
        if ( !out.load(size_t(tileset), orderedSourceFiles, Sc::Terrain::TilesetNames[size_t(tileset)]) )
        {
            std::cerr << "Failed to load tileset data (cv5/vf4)" << std::endl;
            return false;
        }
        return true;
    }

    int generateMap(const std::string & gridPath, const std::string & outputPath, const std::string & starcraftPath, bool noRepairFlag)
    {
        GridSpec spec {};
        if ( !parseGridFile(gridPath, spec) )
            return 1;
        const bool doRepair = !spec.noRepair && !noRepairFlag;

        if ( !loadTerrainDatSilent(starcraftPath) )
        {
            std::cerr << "Failed to load terrain data from: " << starcraftPath << std::endl;
            return 1;
        }
        const auto & tilesetData = terrainDat.get(spec.tileset);

        size_t fillType = findTerrainType(tilesetData, spec.fillName);
        if ( fillType == 0 )
        {
            std::cerr << "Unknown fill terrain: " << spec.fillName << std::endl;
            return 1;
        }

        std::map<char, size_t> terrainBySymbol {};
        for ( const auto & [symbol, name] : spec.legend )
        {
            size_t terrainType = findTerrainType(tilesetData, name);
            if ( terrainType == 0 )
            {
                std::cerr << "Unknown legend terrain: " << name << " (symbol '" << symbol << "')" << std::endl;
                return 1;
            }
            terrainBySymbol[symbol] = terrainType;
        }

        std::cout << "Creating " << spec.width << "x" << spec.height << " " << spec.tilesetName
                  << " map filled with \"" << spec.fillName << "\" (type " << fillType << ")" << std::endl;
        auto mapFile = newFilledMap(spec.tileset, spec.width, spec.height, fillType);
        if ( !mapFile )
        {
            std::cerr << "Failed to create new map" << std::endl;
            return 1;
        }

        ScMap scMap = copyToScMap(*mapFile);
        Chk::IsomCache isomCache(spec.tileset, spec.width, spec.height, tilesetData);

        const size_t isomWidth = scMap.getIsomWidth();   // width/2 + 1
        const size_t isomHeight = scMap.getIsomHeight(); // height + 1
        const size_t gridRowCount = spec.gridRows.size();

        size_t placed = 0, skipped = 0;
        // Valid isom diamonds: x and y parity must match
        for ( size_t y=0; y<isomHeight; ++y )
        {
            const size_t gy = y * gridRowCount / isomHeight;
            const std::string & row = spec.gridRows[gy];
            for ( size_t x=(y%2); x<isomWidth; x+=2 )
            {
                const size_t gx = x * row.size() / isomWidth;
                const char symbol = row[gx];
                auto it = terrainBySymbol.find(symbol);
                if ( it == terrainBySymbol.end() || it->second == fillType )
                {
                    ++skipped;
                    continue;
                }
                if ( scMap.placeIsomTerrain({x, y}, it->second, spec.brushExtent, isomCache) )
                {
                    scMap.updateTilesFromIsom(isomCache);
                    ++placed;
                }
                else
                    ++skipped;
            }
        }
        copyFromScMap(*mapFile, scMap);
        std::cout << "Placed " << placed << " diamonds (" << skipped << " skipped)" << std::endl;

        const std::vector<u16> baseTiles = mapFile->tiles; // 스탬프 이전의 바탕 지형 (carve 복원용)

        std::vector<std::array<size_t,4>> stampRects {}; // (x,y,w,h) of each applied stamp, for repair

        // Stamps: rect-tile(extended terrain) 조각을 ISOM 지형 위에 MTXM으로 기록.
        // (사람 맵퍼의 "등각 먼저, 사각타일 마지막" 순서와 동일. TILE은 등각 캐시라 두지 않음)
        if ( !spec.stamps.empty() )
        {
            std::string gridDir {};
            size_t slash = gridPath.find_last_of("/\\");
            if ( slash != std::string::npos )
                gridDir = gridPath.substr(0, slash + 1);
            for ( const auto & op : spec.stamps )
            {
                std::string stampPath = op.file;
                if ( stampPath.find(':') == std::string::npos && stampPath[0] != '/' && stampPath[0] != '\\' )
                    stampPath = gridDir + stampPath;
                Stamp stamp {};
                if ( !loadStamp(stampPath, stamp) )
                    return 1;
                if ( normalizeName(stamp.tilesetName) != normalizeName(spec.tilesetName) )
                {
                    std::cerr << "Stamp tileset mismatch (" << stamp.tilesetName << " != "
                              << spec.tilesetName << "): " << op.file << std::endl;
                    return 1;
                }
                size_t placedTiles = 0;
                for ( size_t sy = 0; sy < stamp.height; ++sy )
                {
                    for ( size_t sx = 0; sx < stamp.width; ++sx )
                    {
                        size_t tx = op.x + sx, ty = op.y + sy;
                        if ( tx < spec.width && ty < spec.height )
                        {
                            // 사람의 사각타일 편집과 동일하게 게임용(MTXM)과 에디터 표시용(TILE) 모두 기록
                            mapFile->tiles[ty * spec.width + tx] = stamp.tiles[sy * stamp.width + sx];
                            mapFile->editorTiles[ty * spec.width + tx] = stamp.tiles[sy * stamp.width + sx];
                            ++placedTiles;
                        }
                    }
                }
                std::cout << "Stamped " << op.file << " at (" << op.x << "," << op.y << "): "
                          << placedTiles << " tiles" << std::endl;
                stampRects.push_back({ op.x, op.y, stamp.width, stamp.height });
            }
        }

        // Carves: 지정 사각형을 스탬프 이전의 바탕(등각) 지형으로 복원해 통행로 개통
        for ( const auto & op : spec.carves )
        {
            size_t restored = 0;
            for ( size_t cy = op.y; cy < op.y + op.h && cy < spec.height; ++cy )
            {
                for ( size_t cx = op.x; cx < op.x + op.w && cx < spec.width; ++cx )
                {
                    mapFile->tiles[cy * spec.width + cx] = baseTiles[cy * spec.width + cx];
                    mapFile->editorTiles[cy * spec.width + cx] = baseTiles[cy * spec.width + cx];
                    ++restored;
                }
            }
            std::cout << "Carved (" << op.x << "," << op.y << " " << op.w << "x" << op.h << "): "
                      << restored << " tiles restored" << std::endl;
        }

        // Seam repair: fix link mismatches on the perimeter ring of each stamp rect. Runs after
        // carves so carved areas are excluded (carve results are respected). Off via --no-repair.
        if ( doRepair && !stampRects.empty() )
        {
            std::vector<std::array<size_t,4>> carveRects {};
            for ( const auto & c : spec.carves )
                carveRects.push_back({ c.x, c.y, c.w, c.h });
            const std::vector<size_t> targets = stampRingTargets(spec.width, spec.height, stampRects, carveRects);
            Sc::Terrain::Tiles vf4Data {};
            if ( !loadVf4Tiles(spec.tileset, starcraftPath, vf4Data) )
                return 1;
            const size_t changed = repairTiles(*mapFile, tilesetData, vf4Data, targets);
            std::cout << "repair: " << changed << " tiles" << std::endl;
        }

        // Units (UNIT 단락). 좌표는 타일 단위 → 픽셀(타일 중심)
        for ( const auto & op : spec.units )
        {
            int unitType = findUnitType(op.name);
            if ( unitType < 0 )
            {
                std::cerr << "Unknown unit: " << op.name << std::endl;
                return 1;
            }
            for ( size_t i = 0; i < op.count; ++i )
            {
                Chk::Unit unit {};
                unit.xc = u16((op.x + i % 8) * 32 + 16); // 다수 배치는 8마리씩 행 단위로 펼침
                unit.yc = u16((op.y + i / 8) * 32 + 16);
                unit.type = Sc::Unit::Type(unitType);
                unit.owner = u8(op.owner);
                unit.hitpointPercent = 100;
                unit.shieldPercent = 100;
                unit.energyPercent = 100;
                mapFile->addUnit(unit);
            }
            std::cout << "Unit P" << op.owner + 1 << " \"" << Sc::Unit::defaultDisplayNames[unitType]
                      << "\" x" << op.count << " at (" << op.x << "," << op.y << ")" << std::endl;
        }

        // Start locations (유닛 타입 214)
        for ( const auto & op : spec.starts )
        {
            Chk::Unit unit {};
            unit.xc = u16(op.x * 32 + 16);
            unit.yc = u16(op.y * 32 + 16);
            unit.type = Sc::Unit::Type::StartLocation;
            unit.owner = u8(op.owner);
            mapFile->addUnit(unit);
            std::cout << "Start P" << op.owner + 1 << " at (" << op.x << "," << op.y << ")" << std::endl;
        }

        // Locations (MRGN 단락). 타일 → 픽셀 사각형, 이름 문자열 등록
        for ( const auto & op : spec.locationOps )
        {
            Chk::Location location {};
            location.left = u32(op.x * 32);
            location.top = u32(op.y * 32);
            location.right = u32((op.x + op.w) * 32);
            location.bottom = u32((op.y + op.h) * 32);
            location.elevationFlags = 0; // 모든 고도
            size_t locationId = mapFile->addLocation(location);
            mapFile->setLocationName<RawString>(locationId, RawString(op.name));
            std::cout << "Location \"" << op.name << "\" #" << locationId
                      << " (" << op.x << "," << op.y << " " << op.w << "x" << op.h << ")" << std::endl;
        }

        if ( !mapFile->save(outputPath, true) )
        {
            std::cerr << "Failed to save map to: " << outputPath << std::endl;
            return 1;
        }
        std::cout << "Saved: " << outputPath << std::endl;

        // Also save a raw .chk alongside for structural validation (MPQ-less)
        mapFile->setSaveType(SaveType::RemasteredChk);
        const std::string chkPath = outputPath + ".chk";
        if ( mapFile->save(chkPath, true) )
            std::cout << "Saved: " << chkPath << std::endl;
        return 0;
    }
}

namespace
{
    // Canonical tileset names by Sc::Terrain::Tileset index, matching tilesetNames keys
    const char* canonicalTilesetNames[] = {
        "badlands", "platform", "installation", "ashworld", "jungle", "desert", "ice", "twilight"
    };

    // map -> terrain-type grid (gen-compatible format), for building training data.
    // Each grid cell covers a 2x2 tile block; the cell takes a majority vote of the
    // brush terrain types found there (transition/doodad tiles don't vote).
    int extractGrid(const std::string & mapPath, const std::string & outPath, const std::string & starcraftPath)
    {
        auto mapFile = std::make_unique<MapFile>(mapPath);
        if ( !mapFile || mapFile->empty() )
        {
            std::cerr << "Failed to open map: " << mapPath << std::endl;
            return 1;
        }
        if ( !loadTerrainDatSilent(starcraftPath) )
        {
            std::cerr << "Failed to load terrain data from: " << starcraftPath << std::endl;
            return 1;
        }
        const auto tileset = Sc::Terrain::Tileset(size_t(mapFile->getTileset()) % Sc::Terrain_::NumTilesets);
        const auto & tilesetData = terrainDat.get(tileset);
        const size_t w = mapFile->getTileWidth();
        const size_t h = mapFile->getTileHeight();
        const auto & tiles = mapFile->tiles; // MTXM
        if ( tiles.size() < w * h )
        {
            std::cerr << "MTXM smaller than DIM (" << tiles.size() << " < " << w*h << ")" << std::endl;
            return 1;
        }

        // Brush terrain types and a stable letter per brush (by brush list order)
        std::map<uint16_t, char> brushChar {};
        std::map<uint16_t, std::string> brushName {};
        {
            char nextChar = 'A';
            for ( const auto & brush : tilesetData.brushes )
            {
                brushChar[brush.index] = nextChar++;
                brushName[brush.index] = std::string(brush.name);
            }
        }

        auto tileBrushType = [&](size_t tx, size_t ty) -> uint16_t {
            const size_t group = size_t(tiles[ty * w + tx]) / 16;
            if ( group < tilesetData.tileGroups.size() )
            {
                const uint16_t terrainType = tilesetData.tileGroups[group].terrainType;
                if ( brushChar.count(terrainType) > 0 )
                    return terrainType;
            }
            return 0; // transition/doodad/unknown: no vote
        };

        const size_t gw = w / 2, gh = h / 2;
        std::vector<uint16_t> cells(gw * gh, 0);
        for ( size_t cy = 0; cy < gh; ++cy )
        {
            for ( size_t cx = 0; cx < gw; ++cx )
            {
                std::map<uint16_t, size_t> votes {};
                for ( size_t dy = 0; dy < 2; ++dy )
                {
                    for ( size_t dx = 0; dx < 2; ++dx )
                    {
                        uint16_t terrainType = tileBrushType(cx * 2 + dx, cy * 2 + dy);
                        if ( terrainType != 0 )
                            ++votes[terrainType];
                    }
                }
                uint16_t best = 0; size_t bestCount = 0;
                for ( const auto & [terrainType, count] : votes )
                {
                    if ( count > bestCount )
                    {
                        best = terrainType;
                        bestCount = count;
                    }
                }
                cells[cy * gw + cx] = best;
            }
        }

        // Resolve cells with no vote (pure transition areas) from neighbors, then global majority
        for ( int pass = 0; pass < 4; ++pass )
        {
            bool unresolved = false;
            for ( size_t i = 0; i < cells.size(); ++i )
            {
                if ( cells[i] != 0 )
                    continue;
                const size_t cx = i % gw, cy = i / gw;
                uint16_t neighbor = 0;
                if ( cx > 0 && cells[i-1] != 0 ) neighbor = cells[i-1];
                else if ( cx+1 < gw && cells[i+1] != 0 ) neighbor = cells[i+1];
                else if ( cy > 0 && cells[i-gw] != 0 ) neighbor = cells[i-gw];
                else if ( cy+1 < gh && cells[i+gw] != 0 ) neighbor = cells[i+gw];
                if ( neighbor != 0 )
                    cells[i] = neighbor;
                else
                    unresolved = true;
            }
            if ( !unresolved )
                break;
        }
        std::map<uint16_t, size_t> totals {};
        for ( uint16_t cell : cells )
            if ( cell != 0 )
                ++totals[cell];
        if ( totals.empty() )
        {
            std::cerr << "No brush terrain found in map (fully custom terrain?)" << std::endl;
            return 1;
        }
        uint16_t fillType = 0; size_t fillCount = 0;
        for ( const auto & [terrainType, count] : totals )
        {
            if ( count > fillCount )
            {
                fillType = terrainType;
                fillCount = count;
            }
        }
        for ( uint16_t & cell : cells )
            if ( cell == 0 )
                cell = fillType;

        std::ofstream out(outPath);
        if ( !out )
        {
            std::cerr << "Cannot write: " << outPath << std::endl;
            return 1;
        }
        out << "# extracted from: " << mapPath << "\n";
        out << "tileset " << canonicalTilesetNames[size_t(tileset) % Sc::Terrain_::NumTilesets] << "\n";
        out << "size " << w << " " << h << "\n";
        out << "fill " << brushName[fillType] << "\n";
        out << "brush 1\n";
        for ( const auto & [terrainType, letter] : brushChar )
        {
            if ( totals.count(terrainType) > 0 )
                out << "legend " << letter << " " << brushName[terrainType] << "\n";
        }
        out << "grid\n";
        for ( size_t cy = 0; cy < gh; ++cy )
        {
            for ( size_t cx = 0; cx < gw; ++cx )
                out << brushChar[cells[cy * gw + cx]];
            out << "\n";
        }
        std::cout << "Extracted " << gw << "x" << gh << " grid ("
                  << totals.size() << " terrain types) -> " << outPath << std::endl;
        return 0;
    }

    // 맵의 사각 영역 MTXM을 .stamp 파일로 추출 (extended terrain 조각 채집)
    int clipStamp(const std::string & mapPath, size_t x, size_t y, size_t w, size_t h, const std::string & outPath)
    {
        auto mapFile = std::make_unique<MapFile>(mapPath);
        if ( !mapFile || mapFile->empty() )
        {
            std::cerr << "Failed to open map: " << mapPath << std::endl;
            return 1;
        }
        const size_t mapW = mapFile->getTileWidth(), mapH = mapFile->getTileHeight();
        if ( x + w > mapW || y + h > mapH || w == 0 || h == 0 )
        {
            std::cerr << "Clip rect out of bounds (map " << mapW << "x" << mapH << ")" << std::endl;
            return 1;
        }
        const auto tileset = Sc::Terrain::Tileset(size_t(mapFile->getTileset()) % Sc::Terrain_::NumTilesets);
        std::ofstream out(outPath);
        if ( !out )
        {
            std::cerr << "Cannot write: " << outPath << std::endl;
            return 1;
        }
        out << "# clipped from: " << mapPath << " rect(" << x << "," << y << " " << w << "x" << h << ")\n";
        out << "tileset " << canonicalTilesetNames[size_t(tileset)] << "\n";
        out << "size " << w << " " << h << "\n";
        out << "tiles\n";
        out << std::hex;
        for ( size_t sy = 0; sy < h; ++sy )
        {
            for ( size_t sx = 0; sx < w; ++sx )
                out << mapFile->tiles[(y + sy) * mapW + (x + sx)] << (sx + 1 < w ? " " : "");
            out << "\n";
        }
        std::cout << "Clipped " << w << "x" << h << " -> " << outPath << std::endl;
        return 0;
    }

    // 타일 단위 분석 덤프: MTXM 타일값(hex) 행렬 + 등장 그룹의 terrainType/브러시 여부 테이블 출력.
    int tileDump(const std::string & mapPath, const std::string & outPath, const std::string & starcraftPath)
    {
        auto mapFile = std::make_unique<MapFile>(mapPath);
        if ( !mapFile || mapFile->empty() )
        {
            std::cerr << "Failed to open map: " << mapPath << std::endl;
            return 1;
        }
        if ( !loadTerrainDatSilent(starcraftPath) )
        {
            std::cerr << "Failed to load terrain data from: " << starcraftPath << std::endl;
            return 1;
        }
        const auto tileset = Sc::Terrain::Tileset(size_t(mapFile->getTileset()) % Sc::Terrain_::NumTilesets);
        const auto & tilesetData = terrainDat.get(tileset);
        std::set<uint16_t> brushTypes {};
        for ( const auto & brush : tilesetData.brushes )
            brushTypes.insert(brush.index);

        const size_t w = mapFile->getTileWidth(), h = mapFile->getTileHeight();
        std::ofstream out(outPath);
        out << "tileset " << canonicalTilesetNames[size_t(tileset)] << "\n";
        out << "size " << std::dec << w << " " << h << "\n";
        out << "tiles\n" << std::hex;
        std::set<uint16_t> groupsSeen {};
        for ( size_t ty = 0; ty < h; ++ty )
        {
            for ( size_t tx = 0; tx < w; ++tx )
            {
                uint16_t v = mapFile->tiles[ty * w + tx];
                groupsSeen.insert(v / 16);
                out << v << (tx + 1 < w ? " " : "");
            }
            out << "\n";
        }
        out << "groups\n" << std::dec;
        for ( uint16_t g : groupsSeen )
        {
            uint16_t terrainType = g < tilesetData.tileGroups.size() ? tilesetData.tileGroups[g].terrainType : 0;
            out << g << " " << terrainType << " " << (brushTypes.count(terrainType) ? 1 : 0) << "\n";
        }
        std::cout << "Dumped " << w << "x" << h << " tiles, " << groupsSeen.size()
                  << " groups -> " << outPath << std::endl;
        return 0;
    }

    // 미니타일(8x8px) 단위 통행 가능 지도 덤프: VF4 Walkable 플래그 기반.
    // 출력: size W H (타일) 후 4H줄 x 4W문자 ('1'=통행가능 '0'=차단)
    int walkMap(const std::string & mapPath, const std::string & outPath, const std::string & starcraftPath)
    {
        auto mapFile = std::make_unique<MapFile>(mapPath);
        if ( !mapFile || mapFile->empty() )
        {
            std::cerr << "Failed to open map: " << mapPath << std::endl;
            return 1;
        }
        const std::vector<ArchiveFilePtr> orderedSourceFiles = openScDataFilesSilent(starcraftPath);
        if ( orderedSourceFiles.empty() )
        {
            std::cerr << "Failed to open StarCraft data files at: " << starcraftPath << std::endl;
            return 1;
        }
        const auto tileset = Sc::Terrain::Tileset(size_t(mapFile->getTileset()) % Sc::Terrain_::NumTilesets);
        Sc::Terrain::Tiles tilesetData {};
        if ( !tilesetData.load(size_t(tileset), orderedSourceFiles, Sc::Terrain::TilesetNames[size_t(tileset)]) )
        {
            std::cerr << "Failed to load tileset data (cv5/vf4)" << std::endl;
            return 1;
        }
        const size_t w = mapFile->getTileWidth(), h = mapFile->getTileHeight();
        std::ofstream out(outPath);
        out << "size " << w << " " << h << "\n";
        for ( size_t my = 0; my < h * 4; ++my )
        {
            std::string row(w * 4, '0');
            for ( size_t mx = 0; mx < w * 4; ++mx )
            {
                const uint16_t v = mapFile->tiles[(my / 4) * w + (mx / 4)];
                const size_t group = size_t(v) / 16;
                if ( group < tilesetData.tileGroups.size() )
                {
                    const uint16_t megaIdx = tilesetData.tileGroups[group].megaTileIndex[v % 16];
                    if ( size_t(megaIdx) < tilesetData.tileFlags.size()
                         && tilesetData.tileFlags[megaIdx].miniTileFlags[my % 4][mx % 4].isWalkable() )
                    {
                        row[mx] = '1';
                    }
                }
            }
            out << row << "\n";
        }
        std::cout << "Walkmap " << w * 4 << "x" << h * 4 << " minitiles -> " << outPath << std::endl;
        return 0;
    }

    // 실제 게임 그래픽(VR4/VX4/WPE)으로 맵을 24bpp BMP로 렌더.
    // scale: 1=타일당 32px(원본), 2/4/8 = 픽셀 건너뛰기 다운스케일.
    int renderMap(const std::string & mapPath, const std::string & outPath, size_t scale, const std::string & starcraftPath)
    {
        if ( scale != 1 && scale != 2 && scale != 4 && scale != 8 )
        {
            std::cerr << "scale must be 1|2|4|8" << std::endl;
            return 1;
        }
        auto mapFile = std::make_unique<MapFile>(mapPath);
        if ( !mapFile || mapFile->empty() )
        {
            std::cerr << "Failed to open map: " << mapPath << std::endl;
            return 1;
        }
        const std::vector<ArchiveFilePtr> orderedSourceFiles = openScDataFilesSilent(starcraftPath);
        if ( orderedSourceFiles.empty() )
        {
            std::cerr << "Failed to open StarCraft data files at: " << starcraftPath << std::endl;
            return 1;
        }
        const auto tileset = Sc::Terrain::Tileset(size_t(mapFile->getTileset()) % Sc::Terrain_::NumTilesets);
        Sc::Terrain::Tiles tilesetData {};
        if ( !tilesetData.load(size_t(tileset), orderedSourceFiles, Sc::Terrain::TilesetNames[size_t(tileset)]) )
        {
            std::cerr << "Failed to load tileset graphics (cv5/vx4/vr4/wpe)" << std::endl;
            return 1;
        }
        const size_t w = mapFile->getTileWidth(), h = mapFile->getTileHeight();
        const size_t imgW = w * 32 / scale, imgH = h * 32 / scale;
        const size_t rowBytes = (imgW * 3 + 3) & ~size_t(3); // BMP 4바이트 정렬

        std::vector<uint8_t> pixels(rowBytes * imgH, 0);
        for ( size_t py = 0; py < imgH; ++py )
        {
            const size_t mapPy = py * scale;             // 맵 픽셀 y
            const size_t ty = mapPy / 32;
            for ( size_t px = 0; px < imgW; ++px )
            {
                const size_t mapPx = px * scale;
                const size_t tx = mapPx / 32;
                const uint16_t v = mapFile->tiles[ty * w + tx];
                const size_t group = size_t(v) / 16;
                if ( group >= tilesetData.tileGroups.size() )
                    continue;
                const uint16_t megaIdx = tilesetData.tileGroups[group].megaTileIndex[v % 16];
                if ( size_t(megaIdx) >= tilesetData.tileGraphics.size() )
                    continue;
                const auto & mini = tilesetData.tileGraphics[megaIdx]
                    .miniTileGraphics[(mapPy % 32) / 8][(mapPx % 32) / 8];
                const size_t vr4 = mini.vr4Index();
                if ( vr4 >= tilesetData.miniTilePixels.size() )
                    continue;
                size_t subX = mapPx % 8;
                if ( mini.isFlipped() )
                    subX = 7 - subX;
                const uint8_t wpe = tilesetData.miniTilePixels[vr4].wpeIndex[mapPy % 8][subX];
                const auto & color = tilesetData.systemColorPalette[wpe];
                // BMP는 하단행부터: (imgH-1-py)
                uint8_t* dst = &pixels[(imgH - 1 - py) * rowBytes + px * 3];
                dst[0] = color.blue;
                dst[1] = color.green;
                dst[2] = color.red;
            }
        }

        // BMP 헤더 (BITMAPFILEHEADER 14B + BITMAPINFOHEADER 40B)
        std::ofstream out(outPath, std::ios::binary);
        if ( !out )
        {
            std::cerr << "Cannot write: " << outPath << std::endl;
            return 1;
        }
        const uint32_t dataSize = uint32_t(pixels.size());
        const uint32_t fileSize = 54 + dataSize;
        uint8_t header[54] = { 'B', 'M' };
        auto put32 = [&](size_t off, uint32_t val) { std::memcpy(header + off, &val, 4); };
        auto put16 = [&](size_t off, uint16_t val) { std::memcpy(header + off, &val, 2); };
        put32(2, fileSize);
        put32(10, 54);          // pixel data offset
        put32(14, 40);          // info header size
        put32(18, uint32_t(imgW));
        put32(22, uint32_t(imgH));
        put16(26, 1);           // planes
        put16(28, 24);          // bpp
        put32(34, dataSize);
        out.write(reinterpret_cast<const char*>(header), 54);
        out.write(reinterpret_cast<const char*>(pixels.data()), pixels.size());
        std::cout << "Rendered " << imgW << "x" << imgH << " (scale 1/" << scale << ") -> " << outPath << std::endl;
        return 0;
    }

    int dumpChk(const std::string & mapPath, const std::string & outPath)
    {
        auto mapFile = std::make_unique<MapFile>(mapPath);
        if ( !mapFile || mapFile->empty() )
        {
            std::cerr << "Failed to open map: " << mapPath << std::endl;
            return 1;
        }
        mapFile->setSaveType(SaveType::RemasteredChk);
        if ( !mapFile->save(outPath, true) )
        {
            std::cerr << "Failed to save chk to: " << outPath << std::endl;
            return 1;
        }
        std::cout << "Saved: " << outPath << std::endl;
        return 0;
    }

    // Location editor for an EXISTING map, saved IN PLACE (eud-agent location_write).
    // Ops file: one pipe-separated op per line, coordinates in PIXELS (the caller
    // converts tiles), names as raw bytes (passed through to the string pool as-is):
    //   add|<left>|<top>|<right>|<bottom>|<name>
    //   set|<id>|<left>|<top>|<right>|<bottom>
    //   rename|<id>|<name>
    //   del|<id>
    // Invariants: location ids are NEVER renumbered (save with autoDefragment off);
    // id 64 (Anywhere) is protected; any op failure aborts BEFORE save (all-or-nothing).
    int locEdit(const std::string & mapPath, const std::string & opsPath)
    {
        auto mapFile = std::make_unique<MapFile>(mapPath);
        if ( !mapFile || mapFile->empty() )
        {
            std::cerr << "Failed to open map: " << mapPath << std::endl;
            return 1;
        }
        std::ifstream ops(opsPath, std::ios::binary);
        if ( !ops )
        {
            std::cerr << "Failed to open ops file: " << opsPath << std::endl;
            return 1;
        }

        auto splitPipe = [](const std::string & line) {
            std::vector<std::string> fields {};
            size_t start = 0;
            for ( size_t i = 0; i <= line.size(); ++i )
            {
                if ( i == line.size() || line[i] == '|' )
                {
                    fields.push_back(line.substr(start, i - start));
                    start = i + 1;
                }
            }
            return fields;
        };
        auto isBlankLoc = [](const Chk::Location & loc) {
            return loc.left == 0 && loc.top == 0 && loc.right == 0
                && loc.bottom == 0 && loc.stringId == 0;
        };
        // ids are 1-based; numLocations() is the highest addressable id.
        auto validId = [&](size_t id) {
            if ( id < 1 || id > mapFile->numLocations() )
            {
                std::cerr << "location #" << id << " is out of range (1-"
                          << mapFile->numLocations() << ")" << std::endl;
                return false;
            }
            if ( id == size_t(Chk::LocationId::Anywhere) )
            {
                std::cerr << "location #64 (Anywhere) is protected" << std::endl;
                return false;
            }
            return true;
        };

        size_t applied = 0;
        std::string line {};
        try
        {
            while ( std::getline(ops, line) )
            {
                if ( !line.empty() && line.back() == '\r' )
                    line.pop_back();
                if ( line.empty() )
                    continue;
                const auto f = splitPipe(line);
                const std::string & cmd = f[0];
                if ( cmd == "add" && f.size() >= 6 )
                {
                    Chk::Location location {};
                    location.left = u32(std::stoul(f[1]));
                    location.top = u32(std::stoul(f[2]));
                    location.right = u32(std::stoul(f[3]));
                    location.bottom = u32(std::stoul(f[4]));
                    location.elevationFlags = 0; // all elevations
                    size_t id = mapFile->addLocation(location);
                    if ( id == size_t(Chk::LocationId::NoLocation) )
                    {
                        std::cerr << "no free location slot (all "
                                  << mapFile->numLocations() << " in use)" << std::endl;
                        return 1;
                    }
                    mapFile->setLocationName<RawString>(id, RawString(f[5]));
                    std::cout << "OK add #" << id << std::endl;
                }
                else if ( cmd == "set" && f.size() >= 6 )
                {
                    const size_t id = std::stoul(f[1]);
                    if ( !validId(id) )
                        return 1;
                    auto & location = mapFile->getLocation(id);
                    if ( isBlankLoc(location) )
                    {
                        std::cerr << "location #" << id << " is empty (use add)" << std::endl;
                        return 1;
                    }
                    location.left = u32(std::stoul(f[2]));
                    location.top = u32(std::stoul(f[3]));
                    location.right = u32(std::stoul(f[4]));
                    location.bottom = u32(std::stoul(f[5]));
                    std::cout << "OK set #" << id << std::endl;
                }
                else if ( cmd == "rename" && f.size() >= 3 )
                {
                    const size_t id = std::stoul(f[1]);
                    if ( !validId(id) )
                        return 1;
                    if ( isBlankLoc(mapFile->getLocation(id)) )
                    {
                        std::cerr << "location #" << id << " is empty (use add)" << std::endl;
                        return 1;
                    }
                    mapFile->setLocationName<RawString>(id, RawString(f[2]));
                    std::cout << "OK rename #" << id << std::endl;
                }
                else if ( cmd == "del" && f.size() >= 2 )
                {
                    const size_t id = std::stoul(f[1]);
                    if ( !validId(id) )
                        return 1;
                    if ( isBlankLoc(mapFile->getLocation(id)) )
                    {
                        std::cerr << "location #" << id << " is already empty" << std::endl;
                        return 1;
                    }
                    mapFile->deleteLocation(id, true); // only if unused by map triggers
                    if ( !isBlankLoc(mapFile->getLocation(id)) )
                    {
                        std::cerr << "location #" << id
                                  << " is in use by map triggers; not deleted" << std::endl;
                        return 1;
                    }
                    std::cout << "OK del #" << id << std::endl;
                }
                else
                {
                    std::cerr << "bad op line: " << line << std::endl;
                    return 1;
                }
                ++applied;
            }
        }
        catch ( const std::exception & e )
        {
            std::cerr << "bad op line: " << line << " (" << e.what() << ")" << std::endl;
            return 1;
        }
        if ( applied == 0 )
        {
            std::cerr << "ops file contained no operations" << std::endl;
            return 1;
        }
        // Save IN PLACE. lockAnywhere=true keeps #64 intact; autoDefragmentLocations=
        // false is CRITICAL: defragmenting would renumber location ids and break every
        // existing trigger reference.
        if ( !mapFile->save(mapPath, true, true, true, false) )
        {
            std::cerr << "Failed to save map to: " << mapPath << std::endl;
            return 1;
        }
        std::cout << "SAVED " << applied << " ops" << std::endl;
        return 0;
    }

    // Player editor for an EXISTING map, saved IN PLACE (eud-agent player_setup).
    // Ops file: one pipe-separated op per line, coordinates in PIXELS, slots
    // 0-based (0..7 = P1..P8):
    //   start|<slot>|<xc>|<yc>      (move the slot's start-location unit, or add one)
    //   delstart|<slot>             (remove the slot's start-location unit)
    //   controller|<slot>|<human|computer|rescuable|neutral|inactive|closed>
    // controller writes OWNR + IOWN (StrScope::Both). Invariants mirror locedit:
    // any op failure aborts BEFORE save (all-or-nothing); save keeps
    // autoDefragmentLocations=false so location ids never shift as a side effect.
    int playerEdit(const std::string & mapPath, const std::string & opsPath)
    {
        auto mapFile = std::make_unique<MapFile>(mapPath);
        if ( !mapFile || mapFile->empty() )
        {
            std::cerr << "Failed to open map: " << mapPath << std::endl;
            return 1;
        }
        std::ifstream ops(opsPath, std::ios::binary);
        if ( !ops )
        {
            std::cerr << "Failed to open ops file: " << opsPath << std::endl;
            return 1;
        }

        auto splitPipe = [](const std::string & line) {
            std::vector<std::string> fields {};
            size_t start = 0;
            for ( size_t i = 0; i <= line.size(); ++i )
            {
                if ( i == line.size() || line[i] == '|' )
                {
                    fields.push_back(line.substr(start, i - start));
                    start = i + 1;
                }
            }
            return fields;
        };
        auto validSlot = [](size_t slot) {
            if ( slot > 7 )
            {
                std::cerr << "player slot " << slot << " is out of range (0-7)" << std::endl;
                return false;
            }
            return true;
        };
        // The slot's start-location unit index, or numUnits() when absent.
        auto findStart = [&](size_t slot) {
            for ( size_t i = 0; i < mapFile->numUnits(); ++i )
            {
                const Chk::Unit & unit = mapFile->getUnit(i);
                if ( unit.type == Sc::Unit::Type::StartLocation && size_t(unit.owner) == slot )
                    return i;
            }
            return mapFile->numUnits();
        };
        const std::map<std::string, Sc::Player::SlotType> controllers {
            { "human", Sc::Player::SlotType::Human },
            { "computer", Sc::Player::SlotType::Computer },
            { "rescuable", Sc::Player::SlotType::RescuePassive },
            { "neutral", Sc::Player::SlotType::Neutral },
            { "inactive", Sc::Player::SlotType::Inactive },
            { "closed", Sc::Player::SlotType::GameClosed },
        };
        const size_t maxX = mapFile->getTileWidth() * 32;
        const size_t maxY = mapFile->getTileHeight() * 32;

        size_t applied = 0;
        std::string line {};
        try
        {
            while ( std::getline(ops, line) )
            {
                if ( !line.empty() && line.back() == '\r' )
                    line.pop_back();
                if ( line.empty() )
                    continue;
                const auto f = splitPipe(line);
                const std::string & cmd = f[0];
                if ( cmd == "start" && f.size() >= 4 )
                {
                    const size_t slot = std::stoul(f[1]);
                    const size_t xc = std::stoul(f[2]);
                    const size_t yc = std::stoul(f[3]);
                    if ( !validSlot(slot) )
                        return 1;
                    if ( xc >= maxX || yc >= maxY )
                    {
                        std::cerr << "start P" << slot + 1 << " (" << xc << "," << yc
                                  << ") is outside the map (" << maxX << "x" << maxY << " px)" << std::endl;
                        return 1;
                    }
                    const size_t existing = findStart(slot);
                    if ( existing < mapFile->numUnits() )
                    {
                        Chk::Unit & unit = mapFile->getUnit(existing);
                        unit.xc = u16(xc);
                        unit.yc = u16(yc);
                        std::cout << "OK start P" << slot + 1 << " moved" << std::endl;
                    }
                    else
                    {
                        Chk::Unit unit {};
                        unit.xc = u16(xc);
                        unit.yc = u16(yc);
                        unit.type = Sc::Unit::Type::StartLocation;
                        unit.owner = u8(slot);
                        mapFile->addUnit(unit);
                        std::cout << "OK start P" << slot + 1 << " added" << std::endl;
                    }
                }
                else if ( cmd == "delstart" && f.size() >= 2 )
                {
                    const size_t slot = std::stoul(f[1]);
                    if ( !validSlot(slot) )
                        return 1;
                    const size_t existing = findStart(slot);
                    if ( existing >= mapFile->numUnits() )
                    {
                        std::cerr << "no start location for P" << slot + 1 << std::endl;
                        return 1;
                    }
                    mapFile->deleteUnit(existing);
                    std::cout << "OK delstart P" << slot + 1 << std::endl;
                }
                else if ( cmd == "controller" && f.size() >= 3 )
                {
                    const size_t slot = std::stoul(f[1]);
                    if ( !validSlot(slot) )
                        return 1;
                    const auto it = controllers.find(f[2]);
                    if ( it == controllers.end() )
                    {
                        std::cerr << "unknown controller: " << f[2]
                                  << " (human|computer|rescuable|neutral|inactive|closed)" << std::endl;
                        return 1;
                    }
                    mapFile->setSlotType(slot, it->second); // OWNR + IOWN (StrScope::Both)
                    std::cout << "OK controller P" << slot + 1 << " = " << f[2] << std::endl;
                }
                else
                {
                    std::cerr << "bad op line: " << line << std::endl;
                    return 1;
                }
                ++applied;
            }
        }
        catch ( const std::exception & e )
        {
            std::cerr << "bad op line: " << line << " (" << e.what() << ")" << std::endl;
            return 1;
        }
        if ( applied == 0 )
        {
            std::cerr << "ops file contained no operations" << std::endl;
            return 1;
        }
        // Save IN PLACE with the same flags as locedit (autoDefragmentLocations=
        // false: a player edit must never renumber location ids as a side effect).
        if ( !mapFile->save(mapPath, true, true, true, false) )
        {
            std::cerr << "Failed to save map to: " << mapPath << std::endl;
            return 1;
        }
        std::cout << "SAVED " << applied << " ops" << std::endl;
        return 0;
    }

    // Standalone seam repair: scan an existing map for adjacency link mismatches and replace
    // offending tiles with same-terrainType, same-height candidates that match neighbors best,
    // never reducing walkable minitiles. Without a rect, scans the whole map; with [x y w h],
    // only that rect plus its 1-tile ring. Writes both MTXM and TILE, saves out.scx (+ .chk).
    int repairMap(const std::string & inPath, const std::string & outPath,
                  bool hasRect, size_t rx, size_t ry, size_t rw, size_t rh,
                  const std::string & starcraftPath)
    {
        auto mapFile = std::make_unique<MapFile>(inPath);
        if ( !mapFile || mapFile->empty() )
        {
            std::cerr << "Failed to open map: " << inPath << std::endl;
            return 1;
        }
        if ( !loadTerrainDatSilent(starcraftPath) )
        {
            std::cerr << "Failed to load terrain data from: " << starcraftPath << std::endl;
            return 1;
        }
        const auto tileset = Sc::Terrain::Tileset(size_t(mapFile->getTileset()) % Sc::Terrain_::NumTilesets);
        const auto & isomData = terrainDat.get(tileset);
        Sc::Terrain::Tiles vf4Data {};
        if ( !loadVf4Tiles(tileset, starcraftPath, vf4Data) )
            return 1;

        const size_t w = mapFile->getTileWidth(), h = mapFile->getTileHeight();
        std::vector<size_t> targets {};
        if ( hasRect )
        {
            if ( rw == 0 || rh == 0 )
            {
                std::cerr << "Repair rect must have non-zero w/h" << std::endl;
                return 1;
            }
            const size_t x0 = ( rx > 0 ) ? rx - 1 : 0;            // include 1-tile ring
            const size_t y0 = ( ry > 0 ) ? ry - 1 : 0;
            const size_t x1 = std::min(rx + rw + 1, w);
            const size_t y1 = std::min(ry + rh + 1, h);
            for ( size_t y = y0; y < y1; ++y )
                for ( size_t x = x0; x < x1; ++x )
                    targets.push_back(y * w + x);
        }
        else
        {
            targets.reserve(w * h);
            for ( size_t i = 0; i < w * h; ++i )
                targets.push_back(i);
        }

        const size_t changed = repairTiles(*mapFile, isomData, vf4Data, targets);
        std::cout << "repair: " << changed << " tiles" << std::endl;

        if ( !mapFile->save(outPath, true) )
        {
            std::cerr << "Failed to save map to: " << outPath << std::endl;
            return 1;
        }
        std::cout << "Saved: " << outPath << std::endl;
        mapFile->setSaveType(SaveType::RemasteredChk);
        const std::string chkPath = outPath + ".chk";
        if ( mapFile->save(chkPath, true) )
            std::cout << "Saved: " << chkPath << std::endl;
        return 0;
    }
}

int mapGenMain(int argc, char* argv[])
{
    const std::string defaultScPath = "C:\\Program Files (x86)\\StarCraft";
    if ( argc >= 3 && std::string(argv[1]) == "brushes" )
        return listBrushes(argv[2], argc >= 4 ? argv[3] : defaultScPath);
    else if ( argc >= 4 && std::string(argv[1]) == "gen" )
    {
        // gen <grid> <out.scx> [--no-repair] [starcraft-dir]  (flag may appear in either slot)
        bool noRepair = false;
        std::string scPath = defaultScPath;
        for ( int i = 4; i < argc; ++i )
        {
            if ( std::string(argv[i]) == "--no-repair" )
                noRepair = true;
            else
                scPath = argv[i];
        }
        return generateMap(argv[2], argv[3], scPath, noRepair);
    }
    else if ( argc >= 4 && std::string(argv[1]) == "repair" )
    {
        // repair <in.scx> <out.scx> [x y w h] [starcraft-dir]
        bool hasRect = false;
        size_t rx = 0, ry = 0, rw = 0, rh = 0;
        std::string scPath = defaultScPath;
        const std::string firstRepairArg = ( argc > 4 ) ? std::string(argv[4]) : std::string();
        if ( argc >= 8 && !firstRepairArg.empty()
             && std::all_of(firstRepairArg.begin(), firstRepairArg.end(), [](char c){ return std::isdigit(static_cast<unsigned char>(c)); }) )
        {
            hasRect = true;
            rx = std::stoul(argv[4]); ry = std::stoul(argv[5]);
            rw = std::stoul(argv[6]); rh = std::stoul(argv[7]);
            if ( argc >= 9 )
                scPath = argv[8];
        }
        else if ( argc >= 5 )
            scPath = argv[4];
        return repairMap(argv[2], argv[3], hasRect, rx, ry, rw, rh, scPath);
    }
    else if ( argc >= 4 && std::string(argv[1]) == "chk" )
        return dumpChk(argv[2], argv[3]);
    else if ( argc >= 4 && std::string(argv[1]) == "locedit" )
        return locEdit(argv[2], argv[3]);
    else if ( argc >= 4 && std::string(argv[1]) == "playeredit" )
        return playerEdit(argv[2], argv[3]);
    else if ( argc >= 4 && std::string(argv[1]) == "extract" )
        return extractGrid(argv[2], argv[3], argc >= 5 ? argv[4] : defaultScPath);
    else if ( argc >= 8 && std::string(argv[1]) == "clip" )
        return clipStamp(argv[2], std::stoul(argv[3]), std::stoul(argv[4]),
                         std::stoul(argv[5]), std::stoul(argv[6]), argv[7]);
    else if ( argc >= 4 && std::string(argv[1]) == "tiledump" )
        return tileDump(argv[2], argv[3], argc >= 5 ? argv[4] : defaultScPath);
    else if ( argc >= 4 && std::string(argv[1]) == "walkmap" )
        return walkMap(argv[2], argv[3], argc >= 5 ? argv[4] : defaultScPath);
    else if ( argc >= 4 && std::string(argv[1]) == "render" )
        return renderMap(argv[2], argv[3], argc >= 5 ? std::stoul(argv[4]) : 4,
                         argc >= 6 ? argv[5] : defaultScPath);

    std::cout << "isom-poc map generator" << std::endl
              << "Usage:" << std::endl
              << "  IsomTerrain gen <grid-file> <output.scx> [--no-repair] [starcraft-dir]" << std::endl
              << "  IsomTerrain repair <in.scx> <out.scx> [x y w h] [starcraft-dir]" << std::endl
              << "  IsomTerrain chk <map.scx> <output.chk>   (extract raw chk)" << std::endl
              << "  IsomTerrain locedit <map.scx> <ops.txt>  (edit MRGN locations in place)" << std::endl
              << "  IsomTerrain playeredit <map.scx> <ops.txt>  (start locations + OWNR controllers in place)" << std::endl
              << "  IsomTerrain extract <map.scx> <output.grid> [starcraft-dir]" << std::endl
              << "  IsomTerrain clip <map.scx> <x> <y> <w> <h> <out.stamp>" << std::endl
              << "  IsomTerrain tiledump <map.scx> <out.txt> [starcraft-dir]" << std::endl
              << "  IsomTerrain walkmap <map.scx> <out.txt> [starcraft-dir]" << std::endl
              << "  IsomTerrain render <map.scx> <out.bmp> [scale 1|2|4|8] [starcraft-dir]" << std::endl
              << "  IsomTerrain brushes <tileset> [starcraft-dir]" << std::endl
              << "  IsomTerrain test    (run original IsomTerrain tests)" << std::endl;
    return 2;
}
