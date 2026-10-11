unit gtframe;

interface

uses
  Classes, Controls, Forms, StdCtrls;

type
  TGTFrame = class(TFrame)
    Label2: TLabel;
    Edit1: TEdit;
    procedure Edit1Change(Sender: TObject);
  end;

implementation

{$R *.dfm}

procedure TGTFrame.Edit1Change(Sender: TObject);
begin
  Tag := Tag + 1;
end;

end.
