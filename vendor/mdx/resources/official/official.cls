%% official.cls
%% 公文 LaTeX class for mdx
%% 2026/04/26 v1.0
%%
%% 视觉目标：与 mdx 公文 docx emitter 输出一致（仿宋三号 / 1.5 倍行距 /
%% 2em 首行缩进 / 公文页边距 / 居中页脚 "— X —"）。
%%
%% 编号规则在 tex_official.rs 里完成，本类不调用 \section*/\subsection*；
%% 因此这里设置 secnumdepth=-2 屏蔽自动编号。

\NeedsTeXFormat{LaTeX2e}
\ProvidesClass{official}[2026/07/30 v1.4 mdx 公文 markdown class]

%% ============================================================
%% 基类：ctexart（无章）
%% ============================================================
%% ctexart 的 zihao 类选项只接受 {5, -4}；正文 三号 在 \AtBeginDocument 中设置。
\LoadClass[a4paper,UTF8,zihao=-4]{ctexart}

%% ============================================================
%% 包加载
%% ============================================================
\RequirePackage{geometry}
\RequirePackage{fancyhdr}
\RequirePackage{array}
\RequirePackage{graphicx}
\RequirePackage{etoolbox}
\RequirePackage{xcolor}
\RequirePackage{enumitem}
\RequirePackage{paralist}
\RequirePackage{float}      %% figure [H]：图就地排版
\RequirePackage{longtable}  %% 表格可跨页断开，不为"整表完整"制造空页
\RequirePackage{caption}    %% 浮动体/长表格表题样式
\RequirePackage{tabularray} %% longtblr 智能表格（自动列宽、表头续页重复）
\RequirePackage{hyperref}

%% ============================================================
%% 圆圈数字（与 md2tex.cls 一致）
%% 使用 ①-⑳、⑴-⒇ 专门字符，不用 \textcircled 现场画圈
%% ============================================================
%% 字形路由：xeCJK 默认把这些字符交给罗马字体（而非 CJK 字体），
%% 故用 \newfontfamily 设置。字体跟随正文仿宋（与 research 用正文
%% 主字体一个思路），保证圈号与正文字形风格一致。
%% 已知限制：中文字体普遍缺 ⑪-⑳（U+246A-U+2473），L1 列表超过
%% 10 项时这些圈号会缺字（research 的 FZShuSong-Z01 亦同）。
\IfFontExistsTF{仿宋_GB2312}{%
  \newfontfamily\ennumber{仿宋_GB2312}%
}{%
  \IfFontExistsTF{FangSong_GB2312}{%
    \newfontfamily\ennumber{FangSong_GB2312}%
  }{%
    \IfFontExistsTF{FangSong}{%
      \newfontfamily\ennumber{FangSong}%
    }{%
      \newfontfamily\ennumber{FZShuSong-Z01}%
      \ClassWarning{official}{字体 '仿宋_GB2312' 不可用，圈号回退到 FZShuSong-Z01}%
    }%
  }%
}
\let\encircle\ennumber
\newcommand{\textcriclednum}[1]{%
\ennumber \ifcase#1 \or ①\or ②\or ③\or ④\or ⑤\or ⑥\or ⑦\or ⑧\or ⑨\or ⑩%
\or ⑪\or ⑫\or ⑬\or ⑭\or ⑮\or ⑯\or ⑰\or ⑱\or ⑲\or ⑳\else{Error}\fi}
\newcommand{\circlenum}[1]{%
\encircle\ifcase#1 \or ⑴\or ⑵\or ⑶\or ⑷\or ⑸\or ⑹\or ⑺\or ⑻\or ⑼\or ⑽%
\or ⑾\or ⑿\or ⒀\or ⒁\or ⒂\or ⒃\or ⒄\or ⒅\or ⒆\or ⒇\else{Error}\fi}

%% mdx 列表：L1 → asparaenum（首行缩进段落式），L2..6 → inparaenum（行内紧排）
\setdefaultenum{\circlenum{\arabic{enumi}}\hspace{-0.2em}}{\textcriclednum{\arabic{enumii}}\hspace{-0.3em}}{(\Alph{enumiii})}{(\alph{enumiv})}
\setenumerate[5]{label=\Roman*.}
\setenumerate[6]{label=\roman*.}

%% 关闭自动节编号
\setcounter{secnumdepth}{-2}

%% ============================================================
%% 页面
%% ============================================================
\geometry{
  a4paper,
  top=3.7cm,
  bottom=3.5cm,
  left=2.8cm,
  right=2.6cm,
  footskip=1.2cm
}

%% ============================================================
%% 字体（带 \IfFontExistsTF 兜底）
%% ============================================================

%% 等宽字体：西文优先 JetBrains Mono，中文优先方正楷体。公文模板继续保留
%% 系统字体回退，避免仅含正文内容的旧环境因代码字体缺失而失败。
\IfFontExistsTF{JetBrains Mono}{%
  \setmonofont{JetBrains Mono}%
}{%
  \ClassWarning{official}{字体 'JetBrains Mono' 不可用，西文等宽字体使用系统默认}%
}
\IfFontExistsTF{FZKai-Z03}{%
  \setCJKmonofont{FZKai-Z03}%
}{%
  \IfFontExistsTF{楷体_GB2312}{%
    \setCJKmonofont{楷体_GB2312}%
  }{%
    \IfFontExistsTF{KaiTi_GB2312}{%
      \setCJKmonofont{KaiTi_GB2312}%
    }{%
      \IfFontExistsTF{KaiTi}{%
        \setCJKmonofont{KaiTi}%
      }{%
        \ClassWarning{official}{字体 'FZKai-Z03' 不可用，中文等宽字体使用系统默认}%
      }%
    }%
  }%
}

%% 标题：方正小标宋简体 → FZXiaoBiaoSong-B05 → SimSun
\IfFontExistsTF{方正小标宋简体}{%
  \setCJKfamilyfont{xbsong}{方正小标宋简体}%
}{%
  \IfFontExistsTF{FZXiaoBiaoSong-B05}{%
    \setCJKfamilyfont{xbsong}{FZXiaoBiaoSong-B05}%
  }{%
    \setCJKfamilyfont{xbsong}{SimSun}%
    \ClassWarning{official}{字体 '方正小标宋简体' 不可用，回退到 SimSun}%
  }%
}
\NewDocumentCommand{\xbsong}{}{\CJKfamily{xbsong}}

%% 二级：黑体 → SimHei → FZHei-B01
\IfFontExistsTF{黑体}{%
  \setCJKfamilyfont{hei}{黑体}%
}{%
  \IfFontExistsTF{SimHei}{%
    \setCJKfamilyfont{hei}{SimHei}%
  }{%
    \setCJKfamilyfont{hei}{FZHei-B01}%
    \ClassWarning{official}{字体 '黑体' 不可用，回退到 FZHei-B01}%
  }%
}
\NewDocumentCommand{\hei}{}{\CJKfamily{hei}}

%% 三级：楷体_GB2312 → KaiTi_GB2312 → KaiTi → FZKai-Z03
\IfFontExistsTF{楷体_GB2312}{%
  \setCJKfamilyfont{kai}{楷体_GB2312}%
}{%
  \IfFontExistsTF{KaiTi_GB2312}{%
    \setCJKfamilyfont{kai}{KaiTi_GB2312}%
  }{%
    \IfFontExistsTF{KaiTi}{%
      \setCJKfamilyfont{kai}{KaiTi}%
    }{%
      \setCJKfamilyfont{kai}{FZKai-Z03}%
      \ClassWarning{official}{字体 '楷体_GB2312' 不可用，回退到 FZKai-Z03}%
    }%
  }%
}
\NewDocumentCommand{\kai}{}{\CJKfamily{kai}}

%% 正文：仿宋_GB2312 → FangSong_GB2312 → FangSong → FZShuSong-Z01
\IfFontExistsTF{仿宋_GB2312}{%
  \setCJKfamilyfont{fs}{仿宋_GB2312}%
}{%
  \IfFontExistsTF{FangSong_GB2312}{%
    \setCJKfamilyfont{fs}{FangSong_GB2312}%
  }{%
    \IfFontExistsTF{FangSong}{%
      \setCJKfamilyfont{fs}{FangSong}%
    }{%
      \setCJKfamilyfont{fs}{FZShuSong-Z01}%
      \ClassWarning{official}{字体 '仿宋_GB2312' 不可用，回退到 FZShuSong-Z01}%
    }%
  }%
}
\NewDocumentCommand{\fs}{}{\CJKfamily{fs}}

%% ============================================================
%% 正文默认风格：仿宋 三号、1.5 倍行距、2em 首行缩进
%% ============================================================
\AtBeginDocument{\fs\zihao{3}}
\linespread{1.5}\selectfont
\setlength{\parindent}{2em}
\setlength{\parskip}{0pt}

%% ============================================================
%% 紧缩排版
%% ============================================================
%% 页面按内容自然收尾：不再为了撑满版心而拉伸段间距/行间胶水，
%% 宁可页底留白，也不让某一页的段落被抻长。
\raggedbottom
\setlength{\parskip}{0pt plus 0pt minus 0pt}

%% 浮动体（figure [H] 就地排版）前后留白固定且紧凑
\setlength{\floatsep}{6pt plus 1pt minus 1pt}
\setlength{\textfloatsep}{8pt plus 1pt minus 1pt}
\setlength{\intextsep}{6pt plus 1pt minus 1pt}
\setlength{\abovecaptionskip}{4pt}
\setlength{\belowcaptionskip}{4pt}

%% longtable 前后留白紧凑，并允许在页内任意行断开
\setlength{\LTpre}{6pt}
\setlength{\LTpost}{6pt}

%% ============================================================
%% 表题与 longtblr 智能表格（与 research 同一套 emitter）
%% ============================================================
%% 表题编号样式："表 1  标题"，编号黑体四号、题文仿宋四号
\renewcommand{\tablename}{表}
\captionsetup{labelsep=space}

%% 表内文字：四号（14bp/21pt，行距与正文 1.5 倍一致）仿宋，表头黑体居中
\AtBeginEnvironment{longtblr}{\fontsize{14bp}{21pt}\selectfont\fs}
\SetTblrInner[longtblr]{
  cells={font=\fs, halign=c, valign=m},
  row{1}={font=\hei, bg=white}
}
\DefTblrTemplate{contfoot-text}{default}{下一页继续}
\DefTblrTemplate{conthead-text}{default}{（续表）}
\SetTblrStyle{caption-tag}{font=\hei\zihao{4}}
\SetTblrStyle{caption-text}{font=\fs\zihao{4}}

%% 不为避免孤行/表格断开而重排整页：断开的代价远小于抻长版面
\clubpenalty=150
\widowpenalty=150
\displaywidowpenalty=150
\brokenpenalty=100
\setlength{\emergencystretch}{1em}

%% ============================================================
%% 页脚 "— X —"，居中
%% ============================================================
\pagestyle{fancy}
\fancyhf{}
\fancyfoot[C]{\fs\zihao{-4}\textemdash\ \thepage\ \textemdash}
\renewcommand{\headrulewidth}{0pt}
\renewcommand{\footrulewidth}{0pt}

\endinput
