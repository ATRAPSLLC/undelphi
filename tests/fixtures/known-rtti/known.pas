{ Ground-truth RTTI fixture for the `undelphi` crate.

  Every type and member here is deliberately shaped so the golden test
  (tests/known_rtti.rs) can assert EXACT extraction - exact enum value
  names, exact record fields/types, exact property names/types, exact
  ancestry - against a binary whose contents we fully control. Compile with
  several FPC versions / targets to verify accuracy across the matrix:

    fpc -O2 -Twin32 -Pi386      known.pas   -> known.win32.exe
    fpc -O2 -Twin64 -Px86_64    known.pas   -> known.win64.exe

  Keep this source and the golden test in lockstep: if you change a name or
  type here, update the assertions.  See README.md for the build matrix. }
program known;
{$mode objfpc}{$H+}
{$M+}
uses
  typinfo, classes;

type
  { tkEnumeration - exactly four values, declaration order. }
  TColor = (clRed, clGreen, clBlue, clAlpha);

  { tkEnumeration used as a tkSet element type. }
  TColors = set of TColor;

  { tkRecord - four named fields of distinct kinds. }
  TVertex = record
    X: Integer;
    Y: Integer;
    Name: string;
    Tint: TColor;
  end;

  { tkDynArray of a managed element type. }
  TStringArray = array of string;

  { tkMethod - method-pointer (event) type with a known signature. }
  TProgressEvent = procedure(Sender: TObject; Percent: Integer) of object;

  { tkInterface - explicit GUID. }
  IWidget = interface
    ['{1A2B3C4D-5E6F-7081-9223-A1B2C3D4E5F6}']
    function Caption: string;
    procedure Refresh;
  end;

  { tkClass base - published members drive classic + extended RTTI. }
  TShape = class(TPersistent)
  private
    FName: string;
    FColor: TColor;
    FVisible: Boolean;
  published
    property Name: string read FName write FName;
    property Color: TColor read FColor write FColor;
    property Visible: Boolean read FVisible write FVisible;
  end;

  { tkClass derived - adds an event property and an integer property, and a
    published method (event handler shape). }
  TButton = class(TShape)
  private
    FWidth: Integer;
    FOnProgress: TProgressEvent;
  published
    property Width: Integer read FWidth write FWidth;
    property OnProgress: TProgressEvent read FOnProgress write FOnProgress;
    procedure Click(Sender: TObject);
  end;

procedure TButton.Click(Sender: TObject);
begin
end;

{ Force every type's RTTI to be emitted and kept by the linker. }
procedure Anchor;
var
  b: TButton;
  v: TVertex;
  a: TStringArray;
  c: TColors;
begin
  b := TButton.Create;
  v.X := 1; v.Y := 2; v.Name := 'p'; v.Tint := clGreen;
  a := nil; SetLength(a, 1);
  c := [clRed, clBlue];
  Writeln(
    PtrUInt(b.ClassInfo),
    PtrUInt(TypeInfo(TColor)),
    PtrUInt(TypeInfo(TColors)),
    PtrUInt(TypeInfo(TVertex)),
    PtrUInt(TypeInfo(TStringArray)),
    PtrUInt(TypeInfo(TProgressEvent)),
    PtrUInt(TypeInfo(IWidget)),
    Ord(v.Tint) + Length(a) + Ord(clAlpha in c)
  );
  b.Free;
end;

begin
  Anchor;
end.
