// C++Builder ground-truth fixture for the `undelphi` crate.
//
// The C++ counterpart of known.dpr: VCL classes declared in C++ with
// __published sections, so bcc32 emits Delphi-compatible VMTs and RTTI.
// C++Builder names the RTTI of C++ built-in types after the C++ spelling
// (an `int` property's typeinfo is named "int", not "Integer").
//
// TPlainShape is an ordinary C++ class with a virtual method: it has a C++
// vtable, not a Delphi VMT, and must NOT be reported as a class.
//
// Keep this source and tests/programs.rs in lockstep.
#include <vcl.h>
#pragma hdrstop
#include <stdio.h>

typedef void __fastcall (__closure *TCppProgressEvent)(TObject *Sender, int Percent);

class TCppShape : public TPersistent
{
private:
    AnsiString FName;
    bool FVisible;
__published:
    __property AnsiString Name = {read = FName, write = FName};
    __property bool Visible = {read = FVisible, write = FVisible};
};

class TCppButton : public TCppShape
{
private:
    int FWidth;
    TCppProgressEvent FOnProgress;
public:
    virtual void __fastcall Paint();
__published:
    __property int Width = {read = FWidth, write = FWidth};
    __property TCppProgressEvent OnProgress = {read = FOnProgress, write = FOnProgress};
    void __fastcall Click(TObject *Sender);
};

void __fastcall TCppButton::Paint()
{
    FWidth++;
}

void __fastcall TCppButton::Click(TObject *Sender)
{
    FWidth = 0;
}

class TPlainShape
{
public:
    virtual ~TPlainShape() {}
    virtual int Area() { return 42; }
};

int main()
{
    TCppButton *b = new TCppButton;
    TPlainShape *p = new TPlainShape;
    b->Name = "cpp-literal-alpha";
    printf("%s %s %d\n", AnsiString(b->ClassName()).c_str(), b->Name.c_str(), p->Area());
    delete p;
    delete b;
    return 0;
}
