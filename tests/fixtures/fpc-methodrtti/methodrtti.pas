program methodrtti;
{ Fixture to force FPC to emit a populated TVmtMethodExTable (extended
  class-method RTTI with full signatures). Compile with FPC 3.2.2:

    fpc -O2 methodrtti.pas              { -> methodrtti.exe (x86_64-win64) }
    fpc -O2 -Twin32 -Pi386 methodrtti.pas   { 32-bit variant, optional }

  Then copy methodrtti.exe back into tests/samples/ as the fixture for the
  TVmtMethodExTable decoder. The {$RTTI} directive + the TRttiContext walk in
  main() make the compiler emit the extended method table AND keep the linker
  from stripping it. }
{$mode objfpc}{$H+}
{$M+}
uses
  typinfo, rtti, classes;

type
  {$RTTI EXPLICIT METHODS([vcPrivate, vcProtected, vcPublic, vcPublished])}
  TGadget = class(TComponent)
  private
    function Secret(Code: Integer): Boolean;
  protected
    procedure Tick(Sender: TObject);
  public
    constructor Create(AOwner: TComponent; const AName: string); reintroduce;
    procedure DoWork(const Title: string; Count: Integer; var Done: Boolean);
    function Combine(A: Double; B: Double; out Sum: Double): Double;
    procedure Reset;
  end;

function TGadget.Secret(Code: Integer): Boolean;
begin
  Result := Code <> 0;
end;

procedure TGadget.Tick(Sender: TObject);
begin
end;

constructor TGadget.Create(AOwner: TComponent; const AName: string);
begin
  inherited Create(AOwner);
  Name := AName;
end;

procedure TGadget.DoWork(const Title: string; Count: Integer; var Done: Boolean);
begin
  Done := Count > 0;
end;

function TGadget.Combine(A: Double; B: Double; out Sum: Double): Double;
begin
  Sum := A + B;
  Result := Sum;
end;

procedure TGadget.Reset;
begin
end;

var
  ctx: TRttiContext;
  t: TRttiType;
  m: TRttiMethod;
  p: TRttiParameter;
begin
  // Force the extended method RTTI to be linked in and referenced.
  ctx := TRttiContext.Create;
  t := ctx.GetType(TGadget);
  for m in t.GetMethods do
  begin
    Write(m.Name, '(');
    for p in m.GetParameters do
      Write(p.Name, ': ', p.ParamType.Name, '; ');
    Writeln(')');
  end;
  ctx.Free;
end.
