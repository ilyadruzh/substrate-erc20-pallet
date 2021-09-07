# ERC20 Pallet

- [x] mapping(address => uint256) _balances
- [x] mapping(address => mapping(address => uint256)) _allowances
- [x] uint256 _totalSupply
- [ ] string private _name
- [ ] string private _symbol

- [ ] event Transfer(address indexed from, address indexed to, uint256 value);
- [ ] event Approval(address indexed owner, address indexed spender, uint256 value);

- [ ] constructor(string memory name_, string memory symbol_) { _name = name_; _symbol = symbol_; }

- [x] function totalSupply - __total_supply__
- [x] function balanceOf - __balance__
- [x] function transfer - __transfer__
- [ ] function allowance(address owner, address spender) external view returns (uint256);
- [x] function approve - __approve_transfer__;
- [ ] function transferFrom(address sender, address recipient, uint256 amount) external returns (bool);

- [ ] function name() public view virtual override returns (string memory)
- [ ] function symbol() public view virtual override returns (string memory)
- [ ] function decimals() public view virtual override returns (uint8) 

- [ ] function increaseAllowance(address spender, uint256 addedValue) public virtual returns (bool)
- [ ] function decreaseAllowance(address spender, uint256 subtractedValue) public virtual returns (bool)

- [x] function _mint(address account, uint256 amount) - __mint__
- [x] function _burn(address account, uint256 amount) - __burn__
- [ ] function _approve(address owner, address spender, uint256 amount) internal virtual 

- [ ] function _beforeTokenTransfer(address from, address to, uint256 amount) internal virtual 
- [ ] function _afterTokenTransfer(address from, address to, uint256 amount) internal virtual
