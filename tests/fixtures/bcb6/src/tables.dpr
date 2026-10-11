{ VMT-table and property-kind coverage fixture for the `undelphi` crate.

  Populates every table a Delphi 6 VMT header can point at, with members
  whose names, order and shape are known exactly:

    vmtIntfTable    TDispatcher implements ISource + ISink (two GUIDs)
    vmtAutoTable    TAutoThing's `automated` section (dispids 101-103)
    vmtInitTable    TDispatcher's managed fields (string, WideString,
                    Variant, IUnknown, dynamic array, managed record,
                    fixed array of string)
    vmtFieldTable   TOwnerThing's published fields + their class table
    vmtMethodTable  TOwnerThing's published methods
    vmtDynamicTable TDispatcher's dynamic + message methods
    vmtTypeInfo     TAllProps: one published property per type kind, plus
                    index / default / nodefault / stored specifiers and
                    field, static, virtual and indexed accessors

  It also carries data-in-code ground truth: the string literals in
  `Literals` (AnsiString constants the compiler places in CODE ahead of
  the routine) and the resourcestrings in `Res*` (RT_STRING table).

  TLegacy is a Turbo Pascal `object` with a virtual method: its VMT has a
  different, header-less shape and must NOT be reported as a class.

  Keep this source and tests/programs.rs in lockstep. }
program tables;
{$APPTYPE CONSOLE}
uses
  SysUtils, Classes, Messages, TypInfo;

const
  WM_GT_PING = WM_USER + 1;

resourcestring
  ResAlpha = 'undelphi resourcestring alpha';
  ResBeta = 'undelphi resourcestring beta';

type
  { tkEnumeration with a subrange view. }
  TKind = (kAlpha, kBeta, kGamma);
  TKinds = set of TKind;
  TSmall = 1..100;

  { tkMethod - var and out/const modes, and a function result. }
  TKindEvent = procedure(Sender: TObject; Kind: TKind;
    var Handled: Boolean) of object;
  TParseFunc = function(const S: string; out N: Integer): Boolean of object;

  ISource = interface
    ['{11111111-2222-3333-4444-555555555555}']
    function Count: Integer;
  end;

  ISink = interface(ISource)
    ['{66666666-7777-8888-9999-AAAAAAAAAAAA}']
    procedure Push(V: Integer);
  end;

  { Record with managed fields - appears in TDispatcher's init table. }
  TTagged = record
    Id: Integer;
    Tag: string;
  end;

  { One published property per type kind Delphi 6 can publish. }
  TAllProps = class(TPersistent)
  private
    FInt: Integer;
    FByte: Byte;
    FSmall: TSmall;
    FInt64: Int64;
    FChar: Char;
    FWChar: WideChar;
    FShort: ShortString;
    FStr: string;
    FWStr: WideString;
    FVar: Variant;
    FSingle: Single;
    FDouble: Double;
    FExt: Extended;
    FComp: Comp;
    FCurr: Currency;
    FKind: TKind;
    FKinds: TKinds;
    FList: TStrings;
    FOnKind: TKindEvent;
    FOnParse: TParseFunc;
    FSource: ISource;
    FStored: Integer;
    FNoDef: Integer;
    FReadOnly: Integer;
    FSlots: array[0..3] of Integer;
    function GetStr: string;
    procedure SetStr(const Value: string);
    function GetSlot(Index: Integer): Integer;
    procedure SetSlot(Index: Integer; Value: Integer);
  protected
    function GetVirt: Integer; virtual;
  published
    property PInt: Integer read FInt write FInt default 7;
    property PByte: Byte read FByte write FByte;
    property PSmall: TSmall read FSmall write FSmall;
    property PInt64: Int64 read FInt64 write FInt64;
    property PChar: Char read FChar write FChar;
    property PWChar: WideChar read FWChar write FWChar;
    property PShort: ShortString read FShort write FShort;
    property PStr: string read FStr write FStr;
    property PWStr: WideString read FWStr write FWStr;
    property PVar: Variant read FVar write FVar;
    property PSingle: Single read FSingle write FSingle;
    property PDouble: Double read FDouble write FDouble;
    property PExt: Extended read FExt write FExt;
    property PComp: Comp read FComp write FComp;
    property PCurr: Currency read FCurr write FCurr;
    property PKind: TKind read FKind write FKind default kBeta;
    property PKinds: TKinds read FKinds write FKinds default [kAlpha];
    property PList: TStrings read FList write FList;
    property OnKind: TKindEvent read FOnKind write FOnKind;
    property OnParse: TParseFunc read FOnParse write FOnParse;
    property PSource: ISource read FSource write FSource;
    property PStored: Integer read FStored write FStored stored False;
    property PNoDef: Integer read FNoDef write FNoDef nodefault;
    property PReadOnly: Integer read FReadOnly;
    property PMethod: string read GetStr write SetStr;
    property PVirt: Integer read GetVirt;
    property PSlot2: Integer index 2 read GetSlot write SetSlot;
    property PSlot3: Integer index 3 read GetSlot write SetSlot;
  end;

  TPart = class(TComponent)
  end;

  { Published fields (field table + class table) and published methods. }
  TOwnerThing = class(TComponent)
  published
    PartA: TPart;
    PartB: TPart;
    Names: TStringList;
    procedure DoClick(Sender: TObject);
    procedure DoOther(Sender: TObject);
  end;

  { Interfaces, managed fields, dynamic and message methods, virtuals. }
  TDispatcher = class(TInterfacedObject, ISource, ISink)
  private
    FStr: string;
    FWStr: WideString;
    FVar: Variant;
    FUnk: IUnknown;
    FArr: array of string;
    FRec: TTagged;
    FFixed: array[0..2] of string;
    FPlain: Integer;
  protected
    procedure DynA; dynamic;
    procedure DynB; dynamic;
    procedure WMGtPing(var Msg: TMessage); message WM_GT_PING;
    procedure WMPaint(var Msg: TMessage); message WM_PAINT;
  public
    function Count: Integer;
    procedure Push(V: Integer);
    procedure VirtA; virtual;
    procedure Abstract1; virtual; abstract;
    class function Make: TDispatcher;
  end;

  { Overrides one dynamic, one virtual and the abstract slot. }
  TDispatcher2 = class(TDispatcher)
  protected
    procedure DynA; override;
  public
    procedure VirtA; override;
    procedure Abstract1; override;
  end;

  { `automated` section - the legacy OLE automation table. }
  TAutoThing = class(TObject)
  private
    FLevel: Integer;
    function GetLevel: Integer;
    procedure SetLevel(Value: Integer);
  automated
    function Ping(X: Integer): Integer; dispid 101;
    procedure Reset; dispid 102;
    property Level: Integer read GetLevel write SetLevel dispid 103;
  end;

  { Turbo Pascal object type - header-less VMT, not a class. }
  TLegacy = object
    Value: Integer;
    constructor Init;
    procedure Bump; virtual;
  end;

{ TAllProps }

function TAllProps.GetStr: string;
begin
  Result := FStr;
end;

procedure TAllProps.SetStr(const Value: string);
begin
  FStr := Value;
end;

function TAllProps.GetSlot(Index: Integer): Integer;
begin
  Result := FSlots[Index];
end;

procedure TAllProps.SetSlot(Index: Integer; Value: Integer);
begin
  FSlots[Index] := Value;
end;

function TAllProps.GetVirt: Integer;
begin
  Result := FInt * 2;
end;

{ TOwnerThing }

procedure TOwnerThing.DoClick(Sender: TObject);
begin
end;

procedure TOwnerThing.DoOther(Sender: TObject);
begin
end;

{ TDispatcher }

procedure TDispatcher.DynA;
begin
  Inc(FPlain);
end;

procedure TDispatcher.DynB;
begin
  Dec(FPlain);
end;

procedure TDispatcher.WMGtPing(var Msg: TMessage);
begin
  Msg.Result := 1;
end;

procedure TDispatcher.WMPaint(var Msg: TMessage);
begin
  Msg.Result := 0;
end;

function TDispatcher.Count: Integer;
begin
  Result := Length(FArr) + Length(FStr) + Length(FWStr) + FRec.Id;
end;

procedure TDispatcher.Push(V: Integer);
begin
  FPlain := V;
end;

procedure TDispatcher.VirtA;
begin
  FFixed[0] := 'virt-a';
end;

class function TDispatcher.Make: TDispatcher;
begin
  Result := TDispatcher2.Create;
end;

{ TDispatcher2 }

procedure TDispatcher2.DynA;
begin
  inherited DynA;
end;

procedure TDispatcher2.VirtA;
begin
  inherited VirtA;
end;

procedure TDispatcher2.Abstract1;
begin
end;

{ TAutoThing }

function TAutoThing.GetLevel: Integer;
begin
  Result := FLevel;
end;

procedure TAutoThing.SetLevel(Value: Integer);
begin
  FLevel := Value;
end;

function TAutoThing.Ping(X: Integer): Integer;
begin
  Result := X + 1;
end;

procedure TAutoThing.Reset;
begin
  FLevel := 0;
end;

{ TLegacy }

constructor TLegacy.Init;
begin
  Value := 0;
end;

procedure TLegacy.Bump;
begin
  Inc(Value);
end;

{ Data-in-code ground truth: each literal is a distinct AnsiString constant
  of known text and length. dcc32 folds the long concatenation at compile
  time into a single 302-character constant, longer than any ShortString. }
procedure Literals;
var
  s: string;
begin
  s := 'undelphi-literal-alpha';
  Writeln(s);
  s := 'undelphi-literal-beta-0123456789';
  Writeln(s);
  s := StringOfChar('x', 1) + 'undelphi-literal-gamma';
  Writeln(s);
  s := 'undelphi-long-literal-' +
    '0123456789012345678901234567890123456789012345678901234567890123456789' +
    '0123456789012345678901234567890123456789012345678901234567890123456789' +
    '0123456789012345678901234567890123456789012345678901234567890123456789' +
    '0123456789012345678901234567890123456789012345678901234567890123456789';
  Writeln(Length(s));
  Writeln(ResAlpha, ' ', ResBeta);
end;

{ Force every type's RTTI and every class to be kept by the linker. }
procedure Anchor;
var
  p: TAllProps;
  o: TOwnerThing;
  d: TDispatcher;
  a: TAutoThing;
  l: TLegacy;
  m: TMessage;
begin
  p := TAllProps.Create;
  o := TOwnerThing.Create(nil);
  d := TDispatcher.Make;
  a := TAutoThing.Create;
  l.Init;
  l.Bump;
  m.Msg := WM_GT_PING;
  d.Dispatch(m);
  d.DynA;
  d.DynB;
  d.VirtA;
  Writeln(
    Cardinal(p.ClassInfo),
    Cardinal(TypeInfo(TKinds)),
    Cardinal(TypeInfo(TSmall)),
    Cardinal(TypeInfo(TKindEvent)),
    Cardinal(TypeInfo(TParseFunc)),
    Cardinal(TypeInfo(ISink)),
    Cardinal(TypeInfo(TTagged)),
    o.ClassName, d.ClassName, a.Ping(1), l.Value, m.Result
  );
  a.Free;
  o.Free;
  p.Free;
end;

begin
  Literals;
  Anchor;
end.
