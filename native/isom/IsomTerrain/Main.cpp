#include "IsomTests.h"
#include <string>

int mapGenMain(int argc, char* argv[]);

int main(int argc, char* argv[])
{
    if ( argc > 1 && std::string(argv[1]) == "test" )
    {
        testMain();
        return 0;
    }
    return mapGenMain(argc, argv);
}
