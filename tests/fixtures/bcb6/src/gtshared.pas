unit gtshared;

interface

uses
  Classes;

type
  TGTShared = class(TPersistent)
  private
    FCount: Integer;
    FLabel: string;
  published
    property Count: Integer read FCount write FCount;
    property Caption: string read FLabel write FLabel;
  end;

function MakeShared: TGTShared;

implementation

function MakeShared: TGTShared;
begin
  Result := TGTShared.Create;
  Result.Caption := 'from gtpkg';
end;

end.
