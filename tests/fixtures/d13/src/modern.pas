unit modern;

{ Delphi 10.4+ constructs whose DCU records DCU32INT predates: type
  spellings (aliases, generic-instance aliases), XML documentation comments,
  generic instances with nested types and procedural instances, generic
  methods, anonymous methods, an interface spelled through an alias, const
  records passed by reference, and an export name long enough to be
  shortened with an MD5 suffix. }

interface

uses
  System.SysUtils, System.Generics.Collections;

type
  /// <summary>An alias the compiler resolves to Integer.</summary>
  TCount = Integer;

  TPoint3 = record
    X, Y, Z: Double;
  end;

  TCallback = reference to procedure(Value: TCount);

  /// <summary>A generic box.</summary>
  TBox<T> = record
    Value: T;
    function Get: T;
  end;

  TCountBox = TBox<TCount>;

  TPoints = TList<TPoint3>;

  TShapes = class(TInterfacedObject, IUnknown)
  private
    FItems: TList<TCount>;
    FBytes: TBytes;
  public
    constructor Create;
    destructor Destroy; override;
    function Count: TCount;
    procedure Each(const Callback: TCallback);
    function Convert<T>(const Value: T): T;
    property Items: TList<TCount> read FItems;
  end;

var
  GCount: TCount;
  GBytes: TBytes;
  GPoints: TPoints;

/// <summary>Adds two counts.</summary>
/// <param name="A">The first count.</param>
function AddCounts(A, B: TCount): TCount;
function Length3(const P: TPoint3): Double;
function Length3Std(const P: TPoint3): Double; stdcall;
procedure TakeBytes(const B: TBytes);
function MakeCallback: TCallback;
procedure AnExceptionallyLongRoutineNameThatPushesTheMangledExportNameOverTheLimit(
  const FirstParameterWithALongTypeName: TDictionary<string, TList<TPoint3>>;
  const SecondParameterWithALongTypeName: TDictionary<TCount, TList<string>>);

implementation

function TBox<T>.Get: T;
begin
  Result := Value;
end;

constructor TShapes.Create;
begin
  inherited Create;
  FItems := TList<TCount>.Create;
end;

destructor TShapes.Destroy;
begin
  FItems.Free;
  inherited;
end;

function TShapes.Count: TCount;
begin
  Result := FItems.Count;
end;

procedure TShapes.Each(const Callback: TCallback);
var
  I: TCount;
begin
  for I in FItems do
    Callback(I);
end;

function TShapes.Convert<T>(const Value: T): T;
begin
  Result := Value;
end;

function AddCounts(A, B: TCount): TCount;
begin
  Result := A + B;
end;

function Length3(const P: TPoint3): Double;
begin
  Result := Sqrt(P.X * P.X + P.Y * P.Y + P.Z * P.Z);
end;

function Length3Std(const P: TPoint3): Double;
begin
  Result := Length3(P);
end;

procedure TakeBytes(const B: TBytes);
begin
  GBytes := B;
end;

function MakeCallback: TCallback;
begin
  Result :=
    procedure(Value: TCount)
    begin
      GCount := GCount + Value;
    end;
end;

procedure AnExceptionallyLongRoutineNameThatPushesTheMangledExportNameOverTheLimit(
  const FirstParameterWithALongTypeName: TDictionary<string, TList<TPoint3>>;
  const SecondParameterWithALongTypeName: TDictionary<TCount, TList<string>>);
begin
end;

var
  Box: TCountBox;
  Shapes: TShapes;

initialization
  Box.Value := 1;
  GCount := Box.Get;
  Shapes := TShapes.Create;
  GCount := Shapes.Convert<TCount>(GCount) + Shapes.Count;
  Shapes.Free;
end.
