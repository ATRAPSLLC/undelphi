unit modernrt;

{ Binary-side ground truth for the Delphi 2010+ metadata a compiled program
  carries: extended RTTI of every visibility (fields, methods, properties,
  indexed and class properties), attributes with arguments on a class, a
  field, a method, a property and a record (and one taking every argument
  kind an attribute constructor can), records with methods and
  operators, a managed record, generic class instances, an invokable
  interface, an anonymous-method type, a class helper, a scoped
  enumeration, class constructor / destructor, string literals of each
  kind, a resourcestring, a threadvar and a delay-loaded import.
  Everything is reached from Exercise so the linker keeps it. }

interface

uses
  System.SysUtils, System.Classes, System.Generics.Collections, System.TypInfo;

{$RTTI EXPLICIT METHODS([vcPrivate..vcPublished])
  PROPERTIES([vcPrivate..vcPublished]) FIELDS([vcPrivate..vcPublished])}

type
  {$SCOPEDENUMS ON}
  TLevel = (Low, Mid, High);
  {$SCOPEDENUMS OFF}
  TLevels = set of TLevel;

  TagAttribute = class(TCustomAttribute)
  private
    FName: string;
    FValue: Integer;
  public
    constructor Create(const AName: string; AValue: Integer);
    property Name: string read FName;
    property Value: Integer read FValue;
  end;

  MarkAttribute = class(TCustomAttribute)
  end;

  TDigits = set of 0..100;
  TByteSet = set of Byte;

  { Every argument kind an attribute constructor can take: constants of
    each ordinal width, characters, Boolean, Int64, an enumeration, a small
    and a large set, each float, each string kind, a class reference and a
    type-info pointer. }
  ArgsAttribute = class(TCustomAttribute)
  public
    constructor Create(AByte: Byte; AShortInt: ShortInt; AWord: Word;
      ASmall: SmallInt; ACard: Cardinal; AInt: Integer; AInt64: Int64;
      AChar: Char; AAnsiChar: AnsiChar; ABool: Boolean; ALevel: TLevel;
      ALevels: TLevels; ABytes: TByteSet; ASingle: Single; ADouble: Double;
      AExt: Extended; ACurr: Currency; AStr: string; AAnsi: AnsiString;
      AUtf8: UTF8String; AWide: WideString; AShort: ShortString;
      AClass: TClass; AInfo: PTypeInfo);
  end;

  { A set of an anonymous range has no RTTI, so the compiler writes no
    signature for this constructor: its arguments cannot be decoded. }
  DigitsAttribute = class(TCustomAttribute)
  public
    constructor Create(ADigits: TDigits);
  end;

  TPoint3 = record
  private
    FTag: Integer;
  public
    X, Y, Z: Double;
    Name: string;
    constructor Create(AX, AY, AZ: Double);
    function Length: Double;
    class operator Add(const A, B: TPoint3): TPoint3;
    property Tag: Integer read FTag write FTag;
  end;

  [Tag('rec', 7)]
  TTaggedRec = record
    [Mark]
    A: Integer;
    B: string;
  end;

  [Args(200, -100, 60000, -30000, 4000000000, -2000000000, -5000000000,
    'Z', 'q', True, TLevel.High, [TLevel.Low, TLevel.High], [3, 64, 200],
    0.75, 2.5, -1.5, 12.3456, 'unicode ' + #$03BB, 'ansi', 'utf8 ' + #$03BB,
    'wide', 'short', TObject, TypeInfo(TPoint3))]
  TArgHolder = class
  end;

  [Digits([3, 64, 100])]
  TDigitsHolder = class
  end;

  TManaged = record
    Value: Integer;
    class operator Initialize(out Dest: TManaged);
    class operator Finalize(var Dest: TManaged);
  end;

  TBox<T> = class
  private
    FValue: T;
  public
    constructor Create(const AValue: T);
    function Get: T;
    property Value: T read FValue write FValue;
  end;

  ISource = interface(IInvokable)
    ['{3C4D5E6F-7081-4293-A4B5-C6D7E8F90A1B}']
    function Read(Count: Integer; out Data: TBytes): Integer;
    procedure Reset;
  end;

  TNotify = reference to procedure(Sender: TObject; Level: TLevel);

  [Tag('class', 42)]
  TWidget = class(TInterfacedObject, ISource)
  private
    [Mark]
    FCount: Integer;
    FName: string;
    FItems: TList<Integer>;
    FPoint: TPoint3;
    FLevel: TLevel;
    FLevels: TLevels;
    class var FInstances: Integer;
    function GetItem(Index: Integer): Integer;
    procedure SetItem(Index: Integer; const Value: Integer);
  protected
    FRatio: Double;
    procedure DoChange; virtual;
  public
    constructor Create(const AName: string);
    destructor Destroy; override;
    [Tag('method', 3)]
    function Read(Count: Integer; out Data: TBytes): Integer;
    procedure Reset;
    procedure Each(const Callback: TNotify);
    class function Instances: Integer; static;
    class constructor CreateClass;
    class destructor DestroyClass;
    property Items[Index: Integer]: Integer read GetItem write SetItem; default;
    property Point: TPoint3 read FPoint write FPoint;
    class property InstanceCount: Integer read FInstances;
  published
    procedure Changed(Sender: TObject);
    [Tag('prop', 9)]
    property Name: string read FName write FName;
    property Count: Integer read FCount write FCount;
    property Level: TLevel read FLevel write FLevel;
    property Levels: TLevels read FLevels write FLevels;
  end;

  TWidgetHelper = class helper for TWidget
    function Describe: string;
  end;

  EWidgetError = class(Exception);

const
  AnsiText: AnsiString = 'ansi literal';
  Utf8Text: UTF8String = 'utf8 literal ' + #$03BB;
  WideText = 'unicode literal ' + #$03BB;

resourcestring
  SWidgetMissing = 'Widget missing';

threadvar
  TlsCounter: Integer;

function DelayedTick: Cardinal; stdcall;
  external 'kernel32.dll' name 'GetTickCount' delayed;

procedure Exercise;

implementation

constructor TagAttribute.Create(const AName: string; AValue: Integer);
begin
  inherited Create;
  FName := AName;
  FValue := AValue;
end;

constructor ArgsAttribute.Create(AByte: Byte; AShortInt: ShortInt; AWord: Word;
  ASmall: SmallInt; ACard: Cardinal; AInt: Integer; AInt64: Int64;
  AChar: Char; AAnsiChar: AnsiChar; ABool: Boolean; ALevel: TLevel;
  ALevels: TLevels; ABytes: TByteSet; ASingle: Single; ADouble: Double;
  AExt: Extended; ACurr: Currency; AStr: string; AAnsi: AnsiString;
  AUtf8: UTF8String; AWide: WideString; AShort: ShortString;
  AClass: TClass; AInfo: PTypeInfo);
begin
  inherited Create;
end;

constructor DigitsAttribute.Create(ADigits: TDigits);
begin
  inherited Create;
end;

constructor TPoint3.Create(AX, AY, AZ: Double);
begin
  FTag := 0;
  X := AX;
  Y := AY;
  Z := AZ;
  Name := 'p';
end;

function TPoint3.Length: Double;
begin
  Result := Sqrt(X * X + Y * Y + Z * Z);
end;

class operator TPoint3.Add(const A, B: TPoint3): TPoint3;
begin
  Result := TPoint3.Create(A.X + B.X, A.Y + B.Y, A.Z + B.Z);
end;

class operator TManaged.Initialize(out Dest: TManaged);
begin
  Dest.Value := 7;
end;

class operator TManaged.Finalize(var Dest: TManaged);
begin
  Dest.Value := 0;
end;

constructor TBox<T>.Create(const AValue: T);
begin
  inherited Create;
  FValue := AValue;
end;

function TBox<T>.Get: T;
begin
  Result := FValue;
end;

class constructor TWidget.CreateClass;
begin
  FInstances := 0;
end;

class destructor TWidget.DestroyClass;
begin
  FInstances := -1;
end;

constructor TWidget.Create(const AName: string);
begin
  inherited Create;
  FName := AName;
  FItems := TList<Integer>.Create;
  FPoint := TPoint3.Create(1, 2, 3);
  Inc(FInstances);
end;

destructor TWidget.Destroy;
begin
  FItems.Free;
  Dec(FInstances);
  inherited;
end;

function TWidget.GetItem(Index: Integer): Integer;
begin
  Result := FItems[Index];
end;

procedure TWidget.SetItem(Index: Integer; const Value: Integer);
begin
  FItems[Index] := Value;
end;

procedure TWidget.DoChange;
begin
  Inc(FCount);
end;

function TWidget.Read(Count: Integer; out Data: TBytes): Integer;
begin
  SetLength(Data, Count);
  Result := Count;
end;

procedure TWidget.Reset;
begin
  FItems.Clear;
end;

procedure TWidget.Each(const Callback: TNotify);
begin
  Callback(Self, FLevel);
end;

class function TWidget.Instances: Integer;
begin
  Result := FInstances;
end;

procedure TWidget.Changed(Sender: TObject);
begin
  DoChange;
end;

function TWidgetHelper.Describe: string;
begin
  Result := Name + '/' + IntToStr(Count);
end;

procedure Exercise;
var
  W: TWidget;
  Source: ISource;
  Data: TBytes;
  P: TPoint3;
  R: TTaggedRec;
  M: TManaged;
  IntBox: TBox<Integer>;
  StrBox: TBox<string>;
  Total: Integer;
begin
  W := TWidget.Create('widget');
  Source := W;
  Source.Read(4, Data);
  W.FItems.Add(5);
  W[0] := 6;
  W.Each(
    procedure(Sender: TObject; Level: TLevel)
    begin
      TlsCounter := TlsCounter + Ord(Level);
    end);
  P := W.Point + TPoint3.Create(1, 1, 1);
  R.A := Round(P.Length);
  R.B := W.Describe;
  M.Value := M.Value + R.A;
  IntBox := TBox<Integer>.Create(M.Value);
  StrBox := TBox<string>.Create(R.B + string(AnsiText) + string(Utf8Text) + WideText);
  Total := IntBox.Get + System.Length(StrBox.Get) + TWidget.Instances +
    TWidget.InstanceCount + Integer(DelayedTick and 1);
  if Total < 0 then
    raise EWidgetError.Create(SWidgetMissing);
  Writeln(TArgHolder.ClassName, TDigitsHolder.ClassName);
  Writeln(GetTypeName(TypeInfo(TPoint3)), GetTypeName(TypeInfo(TTaggedRec)),
    GetTypeName(TypeInfo(TManaged)), GetTypeName(TypeInfo(TLevels)),
    GetTypeName(TypeInfo(TNotify)), GetTypeName(TypeInfo(ISource)), Total);
  IntBox.Free;
  StrBox.Free;
  Source := nil;
end;

end.
