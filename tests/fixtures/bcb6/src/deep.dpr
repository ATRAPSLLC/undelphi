{ Deeper Delphi 6 coverage fixture for the `undelphi` crate.

  Constructs no other fixture exercises, each with a known shape:

    ISvc          an IInvokable (method-info) interface: Delphi 6 emits method RTTI
                  for it (names, kinds, parameters, result types)
    IDispThing    a dispinterface (tkInterface with ifDispInterface)
    TImpl         implements ISvc with register, stdcall and virtual
                  methods (direct and virtual adjustor thunks), and IExtra
                  by delegation (`implements`) through a field and a getter
    TImpl2        overrides the virtual implementation: its interface thunk
                  must resolve to the override
    TMid          an enumeration subrange (names live in the base type)
    TOddProps     published ByteBool / WordBool / LongBool, string[20],
                  Int64 subrange, Cardinal, Char subrange, enum-subrange,
                  small-set properties
    TArr, TOuter  a static array type and a nested managed record
    TRecArr       a dynamic array of a managed record

  Keep this source and tests/programs.rs in lockstep. }
program deep;
{$APPTYPE CONSOLE}
uses
  SysUtils, Classes, TypInfo;

type
  TLevel = (lvNone, lvLow, lvMid, lvHigh, lvMax);
  TMid = lvLow..lvHigh;
  TNibble = 0..15;
  TNibbles = set of TNibble;
  TName = string[20];
  TBig = -5000000000..5000000000;
  TLetter = 'a'..'z';

  TArr = array[0..2] of string;

  TInner = record
    Code: Integer;
    Text: string;
  end;

  TOuter = record
    Inner: TInner;
    Items: array of TInner;
    Tag: Variant;
    Fixed: TArr;
  end;

  TRecArr = array of TInner;

  { IInvokable is compiled with method info: Delphi 6 emits method RTTI
    for it. }
  ISvc = interface(IInvokable)
    ['{0A1B2C3D-4E5F-6071-8293-A4B5C6D7E8F9}']
    function Add(A, B: Integer): Integer;
    procedure Rename(const Old: string; var New: string; out Count: Integer);
    function Ratio(X: Double): Double; stdcall;
    procedure Reset;
  end;

  IExtra = interface
    ['{11223344-5566-7788-99AA-BBCCDDEEFF00}']
    function Extra: Integer;
  end;

  IMore = interface
    ['{22334455-6677-8899-AABB-CCDDEEFF0011}']
    function More: Integer;
  end;

  IDispThing = dispinterface
    ['{33445566-7788-99AA-BBCC-DDEEFF001122}']
    function Ping(Value: Integer): Integer; dispid 1;
    property Level: Integer dispid 2;
  end;

  TExtra = class(TInterfacedObject, IExtra)
    function Extra: Integer;
  end;

  TMore = class(TInterfacedObject, IMore)
    function More: Integer;
  end;

  TImpl = class(TInterfacedObject, ISvc, IExtra, IMore)
  private
    FExtra: IExtra;
    FMore: TMore;
    function GetMore: IMore;
  public
    constructor Create;
    function Add(A, B: Integer): Integer;
    procedure Rename(const Old: string; var New: string; out Count: Integer);
    function Ratio(X: Double): Double; stdcall;
    procedure Reset; virtual;
    property ExtraImpl: IExtra read FExtra implements IExtra;
    property MoreImpl: IMore read GetMore implements IMore;
  end;

  TImpl2 = class(TImpl)
  public
    procedure Reset; override;
  end;

  TOddProps = class(TPersistent)
  private
    FByteB: ByteBool;
    FWordB: WordBool;
    FLongB: LongBool;
    FName: TName;
    FBig: TBig;
    FCard: Cardinal;
    FLetter: TLetter;
    FMid: TMid;
    FNibbles: TNibbles;
  published
    property ByteB: ByteBool read FByteB write FByteB;
    property WordB: WordBool read FWordB write FWordB;
    property LongB: LongBool read FLongB write FLongB;
    property Name: TName read FName write FName;
    property Big: TBig read FBig write FBig;
    property Card: Cardinal read FCard write FCard;
    property Letter: TLetter read FLetter write FLetter;
    property Mid: TMid read FMid write FMid;
    property Nibbles: TNibbles read FNibbles write FNibbles;
  end;

{ TExtra / TMore }

function TExtra.Extra: Integer;
begin
  Result := 7;
end;

function TMore.More: Integer;
begin
  Result := 8;
end;

{ TImpl }

constructor TImpl.Create;
begin
  inherited Create;
  FExtra := TExtra.Create;
  FMore := TMore.Create;
end;

function TImpl.GetMore: IMore;
begin
  Result := FMore;
end;

function TImpl.Add(A, B: Integer): Integer;
begin
  Result := A + B;
end;

procedure TImpl.Rename(const Old: string; var New: string; out Count: Integer);
begin
  New := Old + New;
  Count := Length(New);
end;

function TImpl.Ratio(X: Double): Double;
begin
  Result := X / 2;
end;

procedure TImpl.Reset;
begin
  FExtra := nil;
end;

procedure TImpl2.Reset;
begin
  inherited Reset;
end;

{ Force every type's RTTI and every class to be kept by the linker. }
procedure Anchor;
var
  svc: ISvc;
  o: TOddProps;
  outer: TOuter;
  recs: TRecArr;
  s: string;
  n: Integer;
begin
  svc := TImpl2.Create;
  s := 'b';
  svc.Rename('a', s, n);
  svc.Reset;
  o := TOddProps.Create;
  outer.Inner.Code := svc.Add(1, 2);
  SetLength(recs, 1);
  recs[0].Text := 'x';
  Writeln(
    Cardinal(o.ClassInfo),
    Cardinal(TypeInfo(TMid)),
    Cardinal(TypeInfo(TNibbles)),
    Cardinal(TypeInfo(TName)),
    Cardinal(TypeInfo(TBig)),
    Cardinal(TypeInfo(TLetter)),
    Cardinal(TypeInfo(TArr)),
    Cardinal(TypeInfo(TOuter)),
    Cardinal(TypeInfo(TRecArr)),
    Cardinal(TypeInfo(ISvc)),
    Cardinal(TypeInfo(IDispThing)),
    Cardinal(TypeInfo(IExtra)),
    Cardinal(TypeInfo(IMore)),
    Cardinal(TypeInfo(ByteBool)),
    svc.Ratio(3), n, outer.Inner.Code, Length(recs)
  );
  o.Free;
end;

begin
  Anchor;
end.
