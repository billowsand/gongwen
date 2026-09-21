%% md2tex.cls
%% Markdown to TeX document class
%% 2026/04/21 v1.0

\NeedsTeXFormat{LaTeX2e}
\ProvidesClass{md2tex}[2026/04/21 v1.0 Markdown to TeX document class]

%% ============================================================
%% 基类
%% ============================================================
%% fontset=none：本类下面把每一处 CJK 字体都显式设定，不用 ctex 按平台探测的
%% 字库。探测结果在 Windows、macOS、Tectonic 上各不相同（分别落到系统字体与
%% fandol），research 样式要求字形可复现，因此这里不留探测余地。\heiti 是本类
%% 唯一用到的 ctex 字体命令，随字体段一起定义。
\LoadClass[linespread=1,openright,twoside,fontset=none]{ctexbook}

%% ============================================================
%% 包加载
%% ============================================================
\RequirePackage{geometry}
\RequirePackage{calc}
\RequirePackage{fancyhdr}
\RequirePackage{enumitem}
\RequirePackage{graphicx}
\RequirePackage{xcolor}
\RequirePackage{booktabs}
\RequirePackage{array}
\RequirePackage{pifont}   %% \ding{72}：封面"密级★年限"的五角星
\RequirePackage{refcount}
\RequirePackage{ifthen}
\RequirePackage{ragged2e}
\RequirePackage{paralist}
\RequirePackage{longtable}
\RequirePackage{etoolbox}
\RequirePackage{pdflscape}
\RequirePackage{float}
\RequirePackage{caption}
\RequirePackage{tabularray}
\RequirePackage{tikz}
\RequirePackage{titlesec}
\RequirePackage{tocloft}
\RequirePackage{listings}
%% 数学公式支持（mathtools 依赖并自动加载 amsmath，显式列出以便预热同步）
\RequirePackage{amsmath}
\RequirePackage{mathtools}

%% hyperref 放在最后，避免冲突
\RequirePackage[unicode,CJKbookmarks=true,bookmarksopen=true,bookmarksnumbered=true,linktoc=all]{hyperref}

%% ============================================================
%% hyperref 设置
%% ============================================================
\hypersetup{
  colorlinks=true,
  linkcolor=black,
  citecolor=black,
  urlcolor=blue,
  pdfstartview=FitH
}

%% ============================================================
%% 字体配置
%% ============================================================
%% 字体有两种来源，由 \MdxFontPath 是否为空决定：
%%
%% - 宿主程序（如公文助手）把随发行包分发的字体放进一个目录并在
%%   \documentclass 之前注入 \MdxFontPath 时，下面按**文件名**从该目录加载。
%%   任何一台机器都不必先安装方正字体，也不会因本机碰巧装了同名字体而排出
%%   另一种字形——这是发行包能"装上就能用"的前提。
%% - 直接用 mdx 命令行时 \MdxFontPath 为空，保持原有行为：按 family name 查询
%%   系统已安装的字体，缺失时明确报错引导安装。
%%
%% 两条分支设定的 family 完全相同，正文宏（\bs \kai \song \fs 等）在分支外
%% 统一定义，改其中一条时另一条必须同步。
\providecommand{\MdxFontPath}{}
%% 不用 \ifx 与 \@empty 比较：\providecommand 定义的空宏是 long macro，与
%% \@empty 的定义属性不同，内容虽为空也会被判成非空。
\if\relax\detokenize\expandafter{\MdxFontPath}\relax
  \newcommand{\mainfont}{FZShuSong-Z01}
  \IfFontExistsTF{FZXiaoBiaoSong-B05}{%
    \newcommand{\maintitle}{FZXiaoBiaoSong-B05}%
  }{%
    %% 部分方正小标宋文件（包括 font/fzxbs.ttf）的西文字体家族名带 S。
    \newcommand{\maintitle}{FZXiaoBiaoSong-B05S}%
  }
  \newcommand{\mainbold}{FZHei-B01}
  \newcommand{\mainitalic}{FZKai-Z03}
  \newcommand{\mainssf}{FZShuSong-Z01}
  \newcommand{\enmainfont}{TeX Gyre Termes}
  \newcommand{\codefont}{JetBrains Mono}
  \newcommand{\codeCJKfont}{FZKai-Z03}

  %% research 样式要求精确使用方正字体，不以开源字体静默替代。命令行模式不
  %% 分发方正字体，因此缺失时给出明确错误，引导用户先安装，而不是等 fontspec
  %% 在正文开始处以较难理解的信息失败。
  \newcommand{\RequireMdxFont}[1]{%
    \IfFontExistsTF{#1}{}{%
      \ClassError{md2tex}{Required font '#1' is not installed}{%
        Install the font into the operating system, then run mdx again.%
      }%
    }%
  }
  \RequireMdxFont{\mainfont}
  \RequireMdxFont{\maintitle}
  \RequireMdxFont{\mainbold}
  \RequireMdxFont{\mainitalic}
  \RequireMdxFont{\codefont}

  \setCJKmainfont[ItalicFont={\mainitalic}, AutoFakeBold=true]{\mainfont}
  \setmainfont{\enmainfont}
  \setmonofont{\codefont}
  \setCJKmonofont{\codeCJKfont}
  \setCJKfamilyfont{xbs}{\maintitle}
  \setCJKfamilyfont{kaiti}{\mainitalic}
  \setCJKfamilyfont{songti}{\mainfont}
  \setCJKfamilyfont{fangsong}{\mainssf}
  \setCJKfamilyfont{heiti}{\mainbold}
  \setCJKfamilyfont{code}{\codeCJKfont}
  \newfontfamily\encircle{\mainfont}
  \newfontfamily\enxbs{\maintitle}
  \newfontfamily\ennumber{\mainfont}
  \newfontfamily\enhei{\mainbold}
  \newfontfamily\codefamily{\codefont}
\else
  %% 文件名是宿主与本类之间的约定，改名等于改协议。西文正文四个字重齐全，
  %% 等宽西文只有 Regular，粗斜体交给 fontspec 合成。
  \setCJKmainfont[Path=\MdxFontPath, ItalicFont={FZKai.ttf}, AutoFakeBold=true]{FZShuSong.ttf}
  \setmainfont[Path=\MdxFontPath,
    BoldFont={texgyretermes-bold.otf},
    ItalicFont={texgyretermes-italic.otf},
    BoldItalicFont={texgyretermes-bolditalic.otf}]{texgyretermes-regular.otf}
  \setmonofont[Path=\MdxFontPath]{JetBrainsMono-Regular.ttf}
  \setCJKmonofont[Path=\MdxFontPath]{FZKai.ttf}
  \setCJKfamilyfont{xbs}[Path=\MdxFontPath]{FZXiaoBiaoSong.ttf}
  \setCJKfamilyfont{kaiti}[Path=\MdxFontPath]{FZKai.ttf}
  \setCJKfamilyfont{songti}[Path=\MdxFontPath]{FZShuSong.ttf}
  \setCJKfamilyfont{fangsong}[Path=\MdxFontPath]{FZShuSong.ttf}
  \setCJKfamilyfont{heiti}[Path=\MdxFontPath]{FZHei.ttf}
  \setCJKfamilyfont{code}[Path=\MdxFontPath]{FZKai.ttf}
  \newfontfamily\encircle[Path=\MdxFontPath]{FZShuSong.ttf}
  \newfontfamily\enxbs[Path=\MdxFontPath]{FZXiaoBiaoSong.ttf}
  \newfontfamily\ennumber[Path=\MdxFontPath]{FZShuSong.ttf}
  \newfontfamily\enhei[Path=\MdxFontPath]{FZHei.ttf}
  \newfontfamily\codefamily[Path=\MdxFontPath]{JetBrainsMono-Regular.ttf}
\fi

\newcommand{\bs}{\CJKfamily{xbs}}
\newcommand{\kai}{\CJKfamily{kaiti}}
\newcommand{\song}{\CJKfamily{songti}}
\newcommand{\fs}{\CJKfamily{fangsong}}
%% fontset=none 下 ctex 不定义 \heiti，本类自己补上；\providecommand 让将来
%% 换回带字库的 fontset 时也不会撞车。
\providecommand{\heiti}{}
\renewcommand{\heiti}{\CJKfamily{heiti}}

%% 代码字体：西文 JetBrains Mono，中文方正楷体。
\newcommand{\codefontstyle}{\CJKfamily{code}\codefamily}

%% ============================================================
%% 标题格式
%% ============================================================
\setcounter{secnumdepth}{3}
\renewcommand{\thesection}{\thechapter.\arabic{section}}
\renewcommand{\thesubsection}{\thechapter.\arabic{section}.\arabic{subsection}}
\renewcommand{\thesubsubsection}{\thechapter.\arabic{section}.\arabic{subsection}.\arabic{subsubsection}}

\titleformat{\section}{\heiti\enhei}{\thesection}{0.5em}{}
\titleformat{\subsection}{\heiti\enhei}{\thesubsection}{0.5em}{}[]
\titleformat{\subsubsection}{\heiti\enhei}{\thesubsubsection}{0.5em}{}[]
\titlespacing{\section}{2em}{0pt}{0pt}
\titlespacing{\subsection}{2em}{0pt}{0pt}
\titlespacing{\subsubsection}{2em}{0pt}{0pt}

%% 章节样式：二号、居中、黑体；编号"第1章"
\ctexset{
  chapter = {
    format = \centering\zihao{-2}\heiti\enhei,
    nameformat = \heiti\enhei,
    numberformat = \heiti\enhei,
    name = {第,章},
    number = \arabic{chapter}
  },
  appendix = {
    name = {附录,},
    number = \Alph{chapter}
  },
}

%% ============================================================
%% 目录格式
%% ============================================================
\renewcommand{\cftchapfont}{\heiti\enhei}
\setlength{\cftbeforechapskip}{\cftbeforesecskip}

%% ============================================================
%% 页面设置
%% ============================================================
\renewcommand\headrulewidth{0pt}
\lhead{} \chead{} \rhead{}
\lfoot{} \cfoot{{\zihao{4} — \ \ennumber\thepage \  — }} \rfoot{}
\pagestyle{fancy}
\fancypagestyle{plain}{
  \lhead{} \chead{} \rhead{}
  \lfoot{} \cfoot{{\zihao{4} — \ \ennumber\thepage \  — }} \rfoot{}
  \renewcommand\headrulewidth{0pt}
}
\geometry{
  a4paper,
  left=28mm plus 1mm minus 1mm,
  height=225mm,
  top=37mm plus 1mm minus 1mm,
  width=156mm,
}

%% ============================================================
%% 浮动体标题
%% ============================================================
\renewcommand\figurename{图}
\renewcommand\tablename{表}
\captionsetup{labelsep=space}

%% ============================================================
%% 列表设置（依赖下面的 \circlenum / \textcriclednum，所以放在它们之后）
%% ============================================================

%% ============================================================
%% 字号与段落
%% ============================================================
\providecommand{\passthrough}[1]{#1}
\renewcommand\normalsize{%
  \@setfontsize\normalsize{14bp}{24pt}%
  \abovedisplayskip 0bp%
  \abovedisplayshortskip 0bp%
  \belowdisplayshortskip 0bp%
  \belowdisplayskip \abovedisplayskip
}
\normalsize
\ifx\MakeRobust\@undefined \else
    \MakeRobust\normalsize
\fi

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

%% 长表格前后留白紧凑；longtblr/longtable 均可跨页断开，
%% 不为"整表完整显示"把表推到下一页而留出大片空白。
\setlength{\LTpre}{6pt}
\setlength{\LTpost}{6pt}

%% 不为避免孤行/表格断开而重排整页：断开的代价远小于抻长版面
\clubpenalty=150
\widowpenalty=150
\displaywidowpenalty=150
\brokenpenalty=100
\setlength{\emergencystretch}{1em}

%% ============================================================
%% 圆圈数字
%% ============================================================
\newcommand{\textcriclednum}[1]{%
\ennumber \ifcase#1 \or ① \or ② \or ③ \or ④ \or ⑤ \or ⑥ \or ⑦ \or ⑧ \or ⑨ \or ⑩ \or ⑪\or ⑫\or ⑬\or ⑭\or ⑮\or ⑯\or ⑰\or ⑱\or ⑲\or ⑳\else{Error}\fi}
\newcommand{\circlenum}[1]{%
\encircle\ifcase#1 \or ⑴\or ⑵\or ⑶\or ⑷\or ⑸\or ⑹\or ⑺\or ⑻\or ⑼\or ⑽\or ⑾\or ⑿\or ⒀\or ⒁\or ⒂\or ⒃\or ⒄\or ⒅\or ⒆\or ⒇\else{Error}\fi}

\setdefaultenum{\circlenum{\arabic{enumi}}\hspace{-0.2em}}{\textcriclednum{\arabic{enumii}}\hspace{-0.3em}}{(\Alph{enumiii})}{(\alph{enumiv})}
\setenumerate[5]{label=\Roman*.}  % L5 (paralist \setenumerate 接受 level)
\setenumerate[6]{label=\roman*.}  % L6

%% ============================================================
%% 计数器
%% ============================================================
\newcounter{Rownumber}
\newcommand{\Rown}{\stepcounter{Rownumber}\theRownumber}
\newcommand{\reRown}{\setcounter{Rownumber}{0}}

\newcounter{zhRownumber}
\newcommand{\zhRown}{\stepcounter{zhRownumber}\zhnumber{\thezhRownumber}}
\newcommand{\rezhRown}{\setcounter{zhRownumber}{0}}

%% ============================================================
%% longtable 环境
%% ============================================================
\let\oldlongtable\longtable
\let\endoldlongtable\endlongtable
\renewenvironment{longtable}[1][c]{
  \oldlongtable[#1]
}{
  \endoldlongtable
}
\AtBeginEnvironment{longtable}{\fontsize{12bp}{18pt}\selectfont\fs\reRown\rezhRown}
\newcommand{\longtablefont}{\fontsize{12bp}{18pt}\selectfont\song}
\newcommand{\longtablesmall}{\fontsize{10bp}{15pt}\selectfont\song}
\newcommand{\longtablelarge}{\fontsize{14bp}{21pt}\selectfont\song}

%% ============================================================
%% longtblr 环境
%% ============================================================
\AtBeginEnvironment{longtblr}{\fontsize{12bp}{18pt}\selectfont\song\reRown\rezhRown}
\SetTblrInner[longtblr]{
  cells={font=\song, halign=c, valign=m},
  row{1}={font=\heiti, bg=white}
}
\DefTblrTemplate{contfoot-text}{default}{下一页继续}
\DefTblrTemplate{conthead-text}{default}{（续表）}
\SetTblrStyle{caption-tag}{font=\heiti\zihao{-4}}
\SetTblrStyle{caption-text}{font=\song\zihao{-4}}

\newcommand{\longtblrfont}{\fontsize{12bp}{18pt}\selectfont\song}
\newcommand{\longtblrsmall}{\fontsize{10bp}{15pt}\selectfont\song}
\newcommand{\longtblrlarge}{\fontsize{14bp}{21pt}\selectfont\song}

%% ============================================================
%% 表格模板命令
%% ============================================================
\newcommand{\celltitle}[1]{\multicolumn{1}{|c|}{\heiti #1}}
\newcommand{\subtitle}[2]{\multicolumn{#2}{|l|}{\textit{\kai（\zhRown ）#1 }}\\\hline}
\newcommand{\tablerow}[1]{\Rown & #1 \\\hline}
\newcommand{\tabletitle}[2]{
\\[-10pt]\hline
\celltitle{\resizebox{1em}{0.85em}{序号}} &
#1 \\\hline
\endfirsthead
\hline
\celltitle{\resizebox{1em}{0.85em}{序号}} &
#1 \\ \hline
\endhead
\hline
\endfoot
\hline
\endlastfoot
}

%% ============================================================
%% 代码配色
%% ============================================================
\definecolor{codebackground}{RGB}{248, 248, 248}
\definecolor{codeborder}{RGB}{220, 220, 220}
\definecolor{keywordcolor}{RGB}{0, 102, 204}
\definecolor{stringcolor}{RGB}{221, 17, 68}
\definecolor{commentcolor}{RGB}{113, 140, 0}
\definecolor{numbercolor}{RGB}{102, 102, 102}
\definecolor{linenumbercolor}{RGB}{153, 153, 153}

\lstset{
    basicstyle=\codefontstyle\fontsize{11bp}{16pt}\selectfont,
    keywordstyle=\color{keywordcolor}\bfseries\codefontstyle,
    stringstyle=\color{stringcolor}\codefontstyle,
    commentstyle=\color{commentcolor}\codefontstyle,
    numberstyle=\tiny\color{numbercolor},
    numbers=left,
    numbersep=10pt,
    numberstyle=\scriptsize\color{linenumbercolor},
    stepnumber=1,
    frame=single,
    frameround=tttt,
    rulecolor=\color{codeborder},
    backgroundcolor=\color{codebackground},
    breaklines=true,
    breakatwhitespace=false,
    tabsize=4,
    showtabs=false,
    showspaces=false,
    showstringspaces=false,
    xleftmargin=20pt,
    xrightmargin=10pt,
    framexleftmargin=15pt,
    framextopmargin=6pt,
    framexbottommargin=6pt,
    captionpos=b,
    emphstyle=\color{red}\bfseries,
    morekeywords={},
    extendedchars=true,
    columns=flexible,
    keepspaces=true,
    aboveskip=10pt,
    belowskip=10pt,
}

\lstdefinestyle{python}{
    language=Python,
    morekeywords={self, True, False, None, as, with, yield, async, await},
}
\lstdefinestyle{cpp}{
    language=C++,
    morekeywords={nullptr, constexpr, decltype, override, final},
}
\lstdefinestyle{java}{
    language=Java,
    morekeywords={var, record},
}
\lstdefinelanguage{Rust}{
    morekeywords={fn, let, mut, pub, use, mod, struct, enum, impl, trait, for, in, loop, while, if, else, match, return, async, await, move, ref, self, super, crate, where, type, as, dyn, const, static, unsafe, extern, true, false},
    sensitive=true,
    comment=[l]{//},
    morecomment=[s]{/*}{*/},
    morestring=[b]",
}
\lstdefinestyle{minimal}{
    numbers=none,
    frame=none,
    xleftmargin=10pt,
    framexleftmargin=0pt,
    backgroundcolor=\color{white},
}

%% ============================================================
%% 杂项命令
%% ============================================================
\newcommand{\xiaokai}[1]{{\kai\zihao{4}【#1】}}
\newcommand{\conditionalHighlight}[1]{%
  \ifnum#1>100
    \textcolor{green}{\textbf{#1}}
  \else
    \textcolor{red}{\textbf{#1}}
  \fi
}

%% ============================================================
%% 摘要环境
%% ============================================================
\newenvironment{abstract}{%
  \chapter*{\centering\heiti\enhei\zihao{-2}摘要}%
  \addcontentsline{toc}{chapter}{摘要}%
  \song%
}{}

%% ============================================================
%% 空白偶数页无页码
%% ============================================================
\newcommand{\clearemptydoublepage}{%
  \clearpage
  \if@twoside
    \ifodd\c@page
    \else
      \thispagestyle{empty}%
      \hbox{}\newpage
      \if@twocolumn\hbox{}\newpage\fi
    \fi
  \fi
}
