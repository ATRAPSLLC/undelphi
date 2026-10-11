object GTFrame: TGTFrame
  Left = 0
  Top = 0
  Width = 320
  Height = 80
  TabOrder = 0
  object Label2: TLabel
    Left = 8
    Top = 8
    Width = 60
    Height = 13
    Caption = 'frame label'
  end
  object Edit1: TEdit
    Left = 8
    Top = 32
    Width = 200
    Height = 21
    TabOrder = 0
    Text = 'frame edit'
    OnChange = Edit1Change
  end
end
