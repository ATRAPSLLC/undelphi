unit dcutypes;

{ DCU fixture: declarations whose full shape exists only in a compiled
  unit. Records without managed fields have no RTTI, routines outside a
  class carry no signature in a binary, private members are invisible to
  Delphi 6 RTTI. tests/dcu.rs reads dcutypes.dcu and checks every
  declaration below. The Size* constants are evaluated by the compiler,
  so the unit states its own type sizes. }

interface

uses
  Classes;

type
  TPlainRec = record
    A: Byte;
    B: Integer;
    C: Word;
    D: Double;
  end;

  TPackedRec = packed record
    A: Byte;
    B: Integer;
    C: Word;
  end;

  TVariantRec = record
    Tag: Integer;
    case Kind: Byte of
      0: (I: Integer);
      1: (D: Double);
      2: (P: Pointer; W: Word);
  end;

  TNested = record
    Inner: TPlainRec;
    Count: Cardinal;
    Name: string[7];
  end;

  TColorKind = (ckRed, ckGreen, ckBlue);
  TColorSet = set of TColorKind;
  TSmall = 10..20;
  TLetters = 'a'..'z';
  TMatrix = array[0..2, 1..3] of Single;
  TShort = string[15];
  PPlainRec = ^TPlainRec;
  TIntArray = array of Integer;
  TFloats = record
    S: Single;
    D: Double;
    E: Extended;
    Cy: Currency;
    Cp: Comp;
    R: Real48;
  end;

  TMyInt = Integer;
  TPlainAlias = TPlainRec;
  TStrongInt = type Integer;

  TCallback = function(A: Integer; const S: string; var R: TPlainRec): Boolean;
  TStdCallback = procedure(X: Integer); stdcall;
  TMethodCallback = procedure(Sender: TObject; Value: Int64) of object;

  IShape = interface
    ['{8E5B6D4A-1C2F-4E3A-9B7D-0A1B2C3D4E5F}']
    function Area: Double;
    procedure Scale(Factor: Double);
  end;

  TBase = class(TObject)
  private
    FPriv: Integer;
    FName: string;
  protected
    FProt: Word;
    procedure VirtProc; virtual;
    procedure DynProc; dynamic;
    function GetName: string; virtual; abstract;
  public
    constructor Create(const AName: string);
    destructor Destroy; override;
    class function ClassFunc(X: Integer): Integer;
    property Name: string read FName write FName;
  end;

  TShape = class(TInterfacedObject, IShape)
  private
    FWidth, FHeight: Double;
    FOnChange: TMethodCallback;
  public
    function Area: Double;
    procedure Scale(Factor: Double);
  published
    property OnChange: TMethodCallback read FOnChange write FOnChange;
  end;

const
  CInt = 42;
  CStr = 'hello';
  CFloat = 3.5;
  CEnum = ckGreen;
  CSet: TColorSet = [ckRed, ckBlue];
  SizePlain = SizeOf(TPlainRec);
  SizePacked = SizeOf(TPackedRec);
  SizeVariant = SizeOf(TVariantRec);
  SizeNested = SizeOf(TNested);
  SizeFloats = SizeOf(TFloats);
  SizeMatrix = SizeOf(TMatrix);
  SizeShort = SizeOf(TShort);
  SizeColorSet = SizeOf(TColorSet);

resourcestring
  SMessage = 'Resource message';

var
  GVar: TPlainRec;
  GPtr: PPlainRec;

threadvar
  TVar: Integer;

function Add(A, B: Integer): Integer;
function AddOverload(A: Integer): Integer; overload;
function AddOverload(A: Double): Double; overload;
procedure DefaultParams(A: Integer; B: Integer = 5; const S: string = 'x');
function StdCallFunc(A: Integer; B: Word): LongBool; stdcall;
function CdeclFunc(A: Integer): Integer; cdecl;
procedure PascalProc(A, B: Integer); pascal;
function SafeCallFunc(A: Integer): Integer; safecall;
procedure VarOut(var A: Integer; out B: Integer; const C: TPlainRec);
function OpenArray(const A: array of Integer): Integer;
function RecResult: TPlainRec;
function ExternalBeep(Freq, Dur: LongWord): LongBool; stdcall;
procedure OldProc; deprecated;
function NestedHost(X: Integer): Integer;

implementation

type
  TImplOnly = record
    X, Y: Integer;
  end;

function ExternalBeep; external 'kernel32.dll' name 'Beep';

function Add(A, B: Integer): Integer;
begin
  Result := A + B;
end;

function AddOverload(A: Integer): Integer;
begin
  Result := A + 1;
end;

function AddOverload(A: Double): Double;
begin
  Result := A + 1;
end;

procedure DefaultParams(A: Integer; B: Integer; const S: string);
begin
  GVar.B := A + B + Length(S);
end;

function StdCallFunc(A: Integer; B: Word): LongBool;
begin
  Result := A > B;
end;

function CdeclFunc(A: Integer): Integer;
begin
  Result := -A;
end;

procedure PascalProc(A, B: Integer);
begin
  GVar.C := A - B;
end;

function SafeCallFunc(A: Integer): Integer;
begin
  Result := A * 2;
end;

procedure VarOut(var A: Integer; out B: Integer; const C: TPlainRec);
begin
  B := A + C.B;
  A := 0;
end;

function OpenArray(const A: array of Integer): Integer;
begin
  Result := Length(A);
end;

function RecResult: TPlainRec;
var
  Local: TImplOnly;
begin
  Local.X := 1;
  Local.Y := 2;
  FillChar(Result, SizeOf(Result), 0);
  Result.B := Local.X + Local.Y;
end;

procedure OldProc;
begin
end;

function NestedHost(X: Integer): Integer;

  function Inner(Y: Integer): Integer;
  begin
    Result := X + Y;
  end;

begin
  Result := Inner(1);
end;

{ TBase }

constructor TBase.Create(const AName: string);
begin
  inherited Create;
  FName := AName;
end;

destructor TBase.Destroy;
begin
  FName := '';
  inherited Destroy;
end;

procedure TBase.VirtProc;
begin
  Inc(FPriv);
end;

procedure TBase.DynProc;
begin
  Inc(FProt);
end;

class function TBase.ClassFunc(X: Integer): Integer;
begin
  Result := X * 3;
end;

{ TShape }

function TShape.Area: Double;
begin
  Result := FWidth * FHeight;
end;

procedure TShape.Scale(Factor: Double);
begin
  FWidth := FWidth * Factor;
  FHeight := FHeight * Factor;
end;

end.
