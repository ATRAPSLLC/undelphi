// Negative control for the `undelphi` crate: a plain Borland C++ 5.6
// console program with no VCL. It carries the Borland C++ runtime but no
// Delphi RTL, so it must not be reported as a Delphi binary with classes.
#include <stdio.h>

class Counter
{
public:
    virtual ~Counter() {}
    virtual int Next() { return ++n; }
private:
    int n;
};

int main()
{
    Counter c;
    printf("plain %d\n", c.Next());
    return 0;
}
