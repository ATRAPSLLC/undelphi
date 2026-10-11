{ Ground-truth RTTI fixture for the `undelphi` crate - Delphi port.

  A line-for-line port of tests/fixtures/known-rtti/known.pas so the same
  ground truth can be asserted against a Borland compiler. Differences from
  the FPC source are only what Delphi 6 requires: no mode directive, Cardinal in
  place of PtrUInt.

  Built twice by compile.sh: statically linked (known.exe) and against the
  rtl runtime package (known_rtl.exe), where TPersistent lives in rtl60.bpl.

  Keep this source and tests/programs.rs in lockstep. }
program known;
{$APPTYPE CONSOLE}
{$M+}
uses
  TypInfo, Classes;

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

  { tkClass base - published members drive classic RTTI. }
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
    Cardinal(b.ClassInfo),
    Cardinal(TypeInfo(TColor)),
    Cardinal(TypeInfo(TColors)),
    Cardinal(TypeInfo(TVertex)),
    Cardinal(TypeInfo(TStringArray)),
    Cardinal(TypeInfo(TProgressEvent)),
    Cardinal(TypeInfo(IWidget)),
    Ord(v.Tint) + Length(a) + Ord(clAlpha in c)
  );
  b.Free;
end;

begin
  Anchor;
end.
