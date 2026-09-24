#include "ErrorHandler.h"

std::atomic<u32> KnownError::nextErrorId(1);

ErrorHandlerResult::~ErrorHandlerResult()
{

}

KnownError::KnownError(u32 id) : errorId(id)
{
    
}

#ifdef _MSC_VER
KnownError::KnownError(const KnownError & other) : std::exception(other.what())
#else // std::exception(const char*) is an MSVC extension
KnownError::KnownError(const KnownError & other) : std::exception(other)
#endif
{
    this->errorId = other.errorId;
}

KnownError::~KnownError()
{

}

u32 KnownError::getErrorId()
{
    return this->errorId;
}

std::atomic<u32> KnownError::GetNextErrorId()
{
    return nextErrorId ++;
}

ErrorHandler::~ErrorHandler()
{

}
